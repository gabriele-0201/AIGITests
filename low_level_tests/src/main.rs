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
        udev::{primary_gpu, UdevBackend},
    },
    reexports::{
        calloop::{timer::Timer, EventLoop, RegistrationToken},
        drm::control::crtc,
        input::Libinput,
        nix::fcntl::OFlag,
    },
    utils::{DeviceFd, Logical, Point},
    wayland::compositor::SurfaceData,
};
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
     */

    let primary_gpu = primary_gpu(&session.seat())
        .unwrap()
        .and_then(|x| {
            DrmNode::from_path(x)
                .ok()?
                .node_with_type(NodeType::Render)?
                .ok()
        })
        .expect("IMP find gpu");
    /*
     * Initialize the udev backend
     */
    let udev_backend = UdevBackend::new(&session.seat()).unwrap();

    let gpus: GpuManager<GbmGlesBackend<GlesRenderer>> =
        GpuManager::new(GbmGlesBackend::default()).unwrap();
    let backends: HashMap<DrmNode, BackendData> = HashMap::new();

    for (device_id, path) in udev_backend.device_list() {
        println!("device found by udev: {device_id:?}, {path:?}");

        match DrmNode::from_dev_id(device_id) {
            Ok(node) => {
                // Get the Raw File Descriptor of the Device
                // (if should be the bridge with the file in the /dev folder?)
                let fd = session
                    .open(
                        &path,
                        OFlag::O_RDWR | OFlag::O_CLOEXEC | OFlag::O_NOCTTY | OFlag::O_NONBLOCK,
                    )
                    .unwrap();

                let fd = DrmDeviceFd::new(unsafe { DeviceFd::from_raw_fd(fd) });

                println!("fd of device: {fd:?}");

                let (drm, notifier) =
                    DrmDevice::new(fd.clone(), true).expect("IMP create DrmDevice");

                let gbm = GbmDevice::new(fd).expect("IMP create Gbm Device");

                let registration_token = event_loop
                    .handle()
                    .insert_source(notifier, move |event, metadata, data| match event {
                        DrmEvent::VBlank(crtc) => {
                            println!("VBlank event, crtc: {crtc:?}, metadata: {metadata:?}")
                            // data.state.frame_finish(node, crtc, metadata);
                        }
                        DrmEvent::Error(error) => {
                            println!("error: {error:?}")
                            // error!("{:?}", error);
                        }
                    })
                    .unwrap();

                let render_node =
                    EGLDevice::device_for_display(&EGLDisplay::new(gbm.clone()).unwrap())
                        .ok()
                        .and_then(|x| x.try_get_render_node().ok().flatten())
                        .unwrap_or(node);

                self.backend_data
                    .gpus
                    .as_mut()
                    .add_node(render_node, gbm.clone())
                    .map_err(DeviceAddError::AddNode)?;

                let backend_data = BackendData {
                    registration_token,
                    gbm,
                    drm,
                    drm_scanner: DrmScanner::new(),
                    render_node,
                    surfaces: HashMap::new(),
                };
                backends.insert(node, backend_data.clone());

                for event in backend_data.drm_scanner.scan_connectors(&device.drm) {
                    match event {
                        DrmScanEvent::Connected {
                            connector,
                            crtc: Some(crtc),
                        } => {
                            //self.connector_connected(node, connector, crtc);
                            println!("Connected: crtc-{crtc:?} connector-{connector:?}");
                        }
                        DrmScanEvent::Disconnected {
                            connector,
                            crtc: Some(crtc),
                        } => {
                            //self.connector_disconnected(node, connector, crtc);
                            println!("Disconnected: crtc-{crtc:?} connector-{connector:?}");
                        }
                        _ => {}
                    }
                }
            }
            Err(err) => {
                println!("Impossible get DrmNode from device {device_id:?}, err: {err}");
            }
        }
    }

    event_loop
        .handle()
        .insert_source(udev_backend, |event, _, _state| {
            println!("new udevEvent: {event:?}");
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(5)), |_, _, _| {
            panic!("Aborted");
        })
        .unwrap();

    event_loop
        .run(None, &mut State {}, |_| {})
        .expect("problem with event loop");
}
