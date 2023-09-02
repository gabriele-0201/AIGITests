use smithay::{
    backend::{
        allocator::gbm::GbmDevice,
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, NodeType},
        egl::{EGLDevice, EGLDisplay},
        input::{InputEvent, KeyboardKeyEvent, PointerMotionEvent},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            gles::GlesRenderer,
            multigpu::{gbm::GbmGlesBackend, GpuManager},
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
    utils::{DeviceFd, Logical, Point},
    wayland::compositor::SurfaceData,
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use std::{collections::HashMap, os::fd::FromRawFd, time::Duration};

struct State {}

struct BackendData {
    surfaces: HashMap<crtc::Handle, SurfaceData>,
    gbm: GbmDevice<DrmDeviceFd>,
    drm: DrmDevice,
    drm_scanner: DrmScanner,
    render_node: DrmNode,
    registration_token: RegistrationToken,
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
    let mut pointer_location: Point<f64, Logical> = (0.0, 0.0).into();

    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, _data| match event {
            InputEvent::Keyboard { event } => {
                let keycode = event.key_code();
                let state = event.state();
                println!("keycode: {keycode}, state {state:?}");
            }
            InputEvent::PointerMotion { event, .. } => {
                pointer_location += event.delta();
                println!("Pointer location: {pointer_location:?}");
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

    let (primary_gpu_path, primary_gpu) = udev::primary_gpu(&session.seat())
        .unwrap()
        .and_then(|x| {
            Some((
                x.clone(),
                DrmNode::from_path(x)
                    .ok()?
                    .node_with_type(NodeType::Render)?
                    .ok()
                    .expect("IMP find gpu"),
            ))
        })
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

        match DrmNode::from_dev_id(device_id) {
            Ok(node) => {}
            Err(err) => {
                println!("Impossible get DrmNode from device {device_id:?}, err: {err}");
            }
        }
    }

    // Open the file descriptor
    let fd = session
        .open(&primary_gpu_path, OFlag::empty())
        .expect("IMP open primary gpu");
    // Wrap the file descriptor into a smithay file
    let fd = DrmDeviceFd::new(unsafe { DeviceFd::from_raw_fd(fd) });
    // Now we can initialize the drm device
    let (drm, event_source) = DrmDevice::new(fd, false).unwrap();
    // Add to the event loop the drm'events
    event_loop
        .handle()
        .insert_source(event_source, |event, _, state| {
            // You will get DrmEvent::VBlank events here,
            // VBlank means that the rendering of given output has compleated and output is ready for a next frame.
            match event {
                DrmEvent::VBlank(_handle) => (),
                DrmEvent::Error(_error) => (),
            }
        });

    let mut drm_scanner: DrmScanner = DrmScanner::default();
    // The following should be called every time Udev::Changed event is fired,
    // to make sure all newly connected outputs are initialized,
    let scan_results = drm_scanner.scan_connectors(&drm);
    for event in scan_results {
        match event {
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
                // let drm_surface = drm.create_surface(crtc, drm_mode, &[connector.handle()]).unwrap();
                // Now we have a surface that can be used to render stuff (usually using GBM)

                // let fmt = smithay::backend::drm::buffer::DrmFourcc::Xrgb8888;
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
            }
            _ => {}
        }
    }

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

    // Start the loop
    event_loop
        .run(None, &mut State {}, |_| {})
        .expect("problem with event loop");
}
