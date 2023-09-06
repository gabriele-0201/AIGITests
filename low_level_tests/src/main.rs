mod pointer;

use pointer::*;
use smithay::{
    backend::{
        allocator::{
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc,
        },
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, GbmBufferedSurface, NodeType},
        egl::{EGLDevice, EGLDisplay},
        input::{InputEvent, KeyboardKeyEvent, PointerMotionEvent},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            gles::GlesRenderer,
            multigpu::{gbm::GbmGlesBackend, GpuManager, MultiRenderer, MultiTexture},
            Bind, Frame, Renderer,
        },
        session::{libseat::LibSeatSession, Session},
        udev::{self, UdevBackend},
    },
    reexports::{
        calloop::{timer::Timer, EventLoop, RegistrationToken},
        drm::control::{crtc, Device, ModeTypeFlags},
        input::Libinput,
        nix::fcntl::OFlag,
    },
    utils::{DeviceFd, Logical, Physical, Point, Rectangle, Transform},
    wayland::compositor::SurfaceData,
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use std::{collections::HashMap, os::fd::FromRawFd, time::Duration};

struct State {
    gpu_manager: GpuManager<GbmGlesBackend<GlesRenderer>>,
    render_node: DrmNode,
    output_device: OutputDevice,
    pointer_location: Point<f64, Logical>,
    pointer_element: PointerElement<MultiTexture>,
}

struct OutputDevice {
    gbm_surface: GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>,
    output_size: (i32, i32),
}

fn main() {
    let mut event_loop: EventLoop<State> = EventLoop::try_new().unwrap();

    /*
     * Initialize session
     */
    let (mut session, notifier) = LibSeatSession::new().unwrap();

    // Not sure why this is needed
    event_loop
        .handle()
        .insert_source(notifier, |_, _, _| {})
        .unwrap();

    /*
     * Initialize libinput backend
     */
    let mut libinput_context =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_context.udev_assign_seat(&session.seat()).unwrap();
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    // first location ALSO in smithay is (0,0)
    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, state| match event {
            InputEvent::Keyboard { event } => {
                let keycode = event.key_code();
                let state = event.state();
                println!("keycode: {keycode}, state {state:?}");
            }
            InputEvent::PointerMotion { event, .. } => {
                state.pointer_location += event.delta();
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                println!("Pointer Motion Absolute: {event:?}");
            }
            InputEvent::PointerButton { event, .. } => {
                println!("Pointer Butto: {event:?}");
            }
            InputEvent::PointerAxis { event, .. } => {
                println!("Pointer Axis: {event:?}");
            }
            _ => println!("Other libinput event: {event:?}"),
        })
        .unwrap();

    /*
     * Initialize the Compositor (primary gpu)
     * Firstly get the path of the primary gpu
     */

    let primary_gpu_path = udev::primary_gpu(&session.seat())
        .expect("IMP find gpu")
        .expect("IMP find gpu");

    /*
     * Initialize the udev backend
     */
    let udev_backend = UdevBackend::new(&session.seat()).unwrap();

    for (device_id, path) in udev_backend.device_list() {
        if path == primary_gpu_path {
            println!("primary gpu founded by udev: {device_id:?}, {path:?}");
            continue;
        }

        println!("device founded by udev,: {device_id:?}, {path:?}");

        /*
        match DrmNode::from_dev_id(device_id) {
            Ok(node) => {}
            Err(err) => {
                println!("Impossible get DrmNode from device {device_id:?}, err: {err}");
            }
        }
        */
    }

    // Open the file descriptor
    let fd = session
        .open(&primary_gpu_path, OFlag::empty())
        .expect("IMP open primary gpu");
    // Wrap the file descriptor into a smithay file
    let fd = DrmDeviceFd::new(unsafe { DeviceFd::from_raw_fd(fd) });
    // Now we can initialize the drm device
    let (drm, drm_event_source) = DrmDevice::new(fd, false).unwrap();

    // Creation of the gbm device and the GbmAllocator
    let gbm = GbmDevice::new(drm.device_fd().clone()).unwrap();
    let gbm_allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );

    // We also want to aquire the node of a device that will be doing the rendering
    // (On typical desktop it will probably equal the node that DrmDevice was created from,
    // but on some ARM setups this is splited into two separate nodes,
    // one for gpu acceleration and one for handling outputs)
    let render_node = EGLDevice::device_for_display(&EGLDisplay::new(gbm.clone()).unwrap())
        .unwrap()
        .try_get_render_node()
        .expect("IMP create render node")
        .expect("IMP create render node");

    let mut gpu_manager: GpuManager<GbmGlesBackend<GlesRenderer>> =
        GpuManager::new(Default::default()).expect("IMP create gpu manager");
    gpu_manager
        .as_mut()
        .add_node(render_node, gbm.clone())
        .expect("IMP add render_node to gpu manager");

    let mut renderer = gpu_manager
        .single_renderer(&render_node)
        .expect("IMP extract renderer");

    let pointer_element = PointerElement::new(&mut renderer);

    let mut drm_scanner: DrmScanner = DrmScanner::default();
    // The following should be called every time Udev::Changed event is fired,
    // to make sure all newly connected outputs are initialized,
    let scan_results = drm_scanner.scan_connectors(&drm);
    let drm_scanner_event = scan_results.iter().next().expect("AT LEAST ONE");

    let (gbm_surface, output_size) = match drm_scanner_event {
        DrmScanEvent::Connected {
            connector,
            crtc: Some(crtc),
        } => {
            // Monitors have diferent modes that can be selected, eg. 1080x1920@90hz
            // let's choose the preferred one
            let mode_id = connector
                .modes()
                .iter()
                .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
                .unwrap_or(0);

            let drm_mode = connector.modes()[mode_id];

            /* Allocate a dumb buffer and draw a solid color in it
            let fmt = smithay::backend::allocator::Fourcc::Xrgb8888;
            let mut db = drm
                .create_dumb_buffer(
                    (drm_mode.size().0.into(), drm_mode.size().1.into()),
                    fmt,
                    32,
                )
                .expect("Could not create dumb buffer");

            {
                let mut map = drm
                    .map_dumb_buffer(&mut db)
                    .expect("Could not map dumbbuffer");
                for b in map.as_mut().chunks_exact_mut(4) {
                    // XRGB = XXXX XXXX RRRR RRRR GGGG GGGG BBBB BBBB
                    b[0] = 0;
                    b[1] = 0xff;
                    b[2] = 0;
                    b[3] = 0;
                }
            }
            let fb = drm
                .add_framebuffer(&db, 24, 32)
                .expect("Could not create FB");

            drm.set_crtc(
                crtc,
                Some(fb),
                (0, 0),
                &[connector.handle()],
                Some(drm_mode),
            )
            .expect("Could not set CRTC");
            */

            let drm_surface = drm
                .create_surface(crtc, drm_mode, &[connector.handle()])
                .unwrap();
            // Now we have a surface that can be used to render stuff (usually using GBM)

            // Creation of the gbm surface
            let mut gbm_surface = GbmBufferedSurface::new(
                drm_surface,
                gbm_allocator.clone(),
                //&[Fourcc::Abgr8888, Fourcc::Argb8888],
                &[Fourcc::Xrgb8888],
                renderer
                    .as_mut()
                    .egl_context()
                    .dmabuf_render_formats()
                    .clone(),
            )
            .expect("IMP creating gbm surface");

            let output_size = (drm_mode.size().0 as i32, drm_mode.size().1 as i32);

            draw_frame(
                &mut gbm_surface,
                &mut gpu_manager,
                &render_node,
                output_size,
                (0.0, 0.0).into(),
                &pointer_element,
            );

            // After this you will get VBlank event, in response to it you can render next frame
            (gbm_surface, output_size)
        }
        _ => unimplemented!(),
    };

    // Add to the event loop the drm'events
    event_loop
        .handle()
        .insert_source(drm_event_source, |event, _, state| {
            // You will get DrmEvent::VBlank events here,
            // VBlank means that the rendering of given output has compleated and output is ready for a next frame.
            match event {
                DrmEvent::VBlank(_crtc) => {
                    state.output_device.gbm_surface.frame_submitted().unwrap();
                    draw_frame(
                        &mut state.output_device.gbm_surface,
                        &mut state.gpu_manager,
                        &state.render_node,
                        state.output_device.output_size,
                        state.pointer_location,
                        &state.pointer_element,
                    );
                }
                DrmEvent::Error(_error) => (),
            }
        });

    // Insert in the Loop Udev Events callback
    event_loop
        .handle()
        .insert_source(udev_backend, |event, _, _state| {
            println!("new udevEvent: {event:?}");
        })
        .unwrap();

    // Insert timer in the loop
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(5)), |_, _, _| {
            panic!("Aborted");
        })
        .unwrap();

    let mut state = State {
        pointer_element: PointerElement::new(
            &mut gpu_manager.single_renderer(&render_node).unwrap(),
        ),
        gpu_manager,
        render_node,
        output_device: OutputDevice {
            gbm_surface,
            output_size,
        },
        pointer_location: (0.0, 0.0).into(),
    };

    // Start the loop
    event_loop
        .run(None, &mut state, |_| {})
        .expect("problem with event loop");
}

fn draw_frame(
    gbm_surface: &mut GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>,
    gpu_manager: &mut GpuManager<GbmGlesBackend<GlesRenderer>>,
    render_node: &DrmNode,
    output_size: (i32, i32),
    pointer_location: Point<f64, Logical>,
    pointer_element: &PointerElement<MultiTexture>,
) {
    // Render first frame:
    let (dmabuf, _age) = gbm_surface
        .next_buffer()
        .expect("IMP get next buffer to create the frame");
    // After this call our OpenGl render will render to this buffer
    let mut renderer = gpu_manager.single_renderer(render_node).unwrap();

    renderer.bind(dmabuf).unwrap();

    let mut frame = renderer
        .render(output_size.into(), Transform::Normal)
        .expect("IMP get frame");
    // Draw a solid color to the current target at the specified destination with the specified color.

    let screen = Rectangle::from_loc_and_size((0, 0), output_size);
    let mouse_rect: Rectangle<i32, Physical> = Rectangle::from_loc_and_size(
        (pointer_location.x as i32, pointer_location.y as i32),
        (200, 200),
    );
    frame.clear([0.0, 0.0, 0.0, 0.0], &[screen]).unwrap();
    /* Not draw any more the solid rectangle
    frame
        .draw_solid(mouse_rect, &[screen], [0.0, 1.0, 1.0, 1.0])
        .unwrap();
    */

    frame.render_texture_at(
        &pointer_element.texture,
        (pointer_location.x as i32, pointer_location.y as i32).into(),
        1,
        1.0,
        Transform::Normal,
        &[screen],
        1.0,
    );

    // Frame is done let's submit it
    gbm_surface
        .queue_buffer(None, Some(vec![]), ())
        .expect("IMP submit frame");
}
