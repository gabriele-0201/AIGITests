use std::{os::fd::FromRawFd, time::Duration};

use smithay::{
    backend::{
        drm::{DrmDeviceFd, DrmNode},
        input::{InputEvent, KeyboardKeyEvent},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        session::{libseat::LibSeatSession, Session},
        udev::UdevBackend,
    },
    reexports::{
        calloop::{timer::Timer, EventLoop},
        input::Libinput,
        nix::fcntl::OFlag,
        wayland_server::Display,
    },
    utils::DeviceFd,
};

struct State {}

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

    // Skip the SETUP of the primary_gpu for now

    /*
     * Initialize the udev backend
     */
    let udev_backend = UdevBackend::new(&session.seat()).unwrap();

    for (device_id, path) in udev_backend.device_list() {
        println!("device found by udev: {device_id:?}, {path:?}");

        if let Ok(node) = DrmNode::from_dev_id(device_id) {
            // Get the Raw File Descriptor of the Device
            // (if should be the bridge with the file in the /dev folder?)
            let fd = session
                .open(
                    &path,
                    OFlag::O_RDWR | OFlag::O_CLOEXEC | OFlag::O_NOCTTY | OFlag::O_NONBLOCK,
                )
                .unwrap();

            //
            let fd = DrmDeviceFd::new(unsafe { DeviceFd::from_raw_fd(fd) });

            println!("fd of device: {fd:?}");

            // let (drm, drm_notifier) = drm::DrmDevice::new(fd, false).unwrap();
        }
    }

    event_loop
        .handle()
        .insert_source(udev_backend, |event, _, _state| {
            println!("new udevEvent: {event:?}");
        })
        .unwrap();

    /*
     * Initialize libinput backend
     */
    let mut libinput_context =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_context.udev_assign_seat(&session.seat()).unwrap();
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, _data| match event {
            InputEvent::Keyboard { event } => {
                let keycode = event.key_code();
                let state = event.state();
                println!("keycode: {keycode}, state {state:?}");
            }
            InputEvent::PointerMotion { event, .. } => {
                println!("Pointer Motion: {event:?}");
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
