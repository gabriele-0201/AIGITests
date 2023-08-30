use std::time::Duration;

use smithay::{
    backend::{
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        session::{libseat::LibSeatSession, Session},
        udev::UdevBackend,
    },
    reexports::{
        calloop::{timer::Timer, EventLoop},
        input::Libinput,
        wayland_server::Display,
    },
};

struct State {}

fn main() {
    let mut event_loop: EventLoop<State> = EventLoop::try_new().unwrap();

    /*
     * Initialize session
     */
    let (session, notifier) = LibSeatSession::new().unwrap();

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
