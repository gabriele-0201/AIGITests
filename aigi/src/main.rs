mod backend;
mod input_handler;
mod pointer;
mod render;
mod state;
mod tiling;

use backend::BackendData;
use input_handler::{handle_input, Action};
use pointer::{PointerElement, PointerRenderElement};
use state::{AIGIState, ClientState};

use anyhow::{Error, Result};
use smithay::{
    backend::{
        drm::DrmEvent,
        input::{AbsolutePositionEvent, Event, InputEvent, KeyState, KeyboardKeyEvent},
        renderer::{
            damage::OutputDamageTracker,
            element::{surface::WaylandSurfaceRenderElement, AsRenderElements},
            gles::{GlesRenderer, GlesTexture},
            Bind,
        },
        winit::{self, WinitEvent},
    },
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::{layer_map_for_output, space::render_output, Space, Window, WindowSurfaceType},
    input::{
        keyboard::{keysyms, FilterResult},
        pointer::MotionEvent,
        Seat, SeatHandler, SeatState,
    },
    output::{self, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            generic::Generic,
            timer::{TimeoutAction, Timer},
            EventLoop, Interest, Mode, PostAction,
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::ClientData,
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
            Client, Display, DisplayHandle,
        },
    },
    utils::{Scale, Transform, SERIAL_COUNTER},
    wayland::{
        buffer::BufferHandler,
        compositor::{with_states, CompositorClientState, CompositorHandler, CompositorState},
        data_device::{
            ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
        },
        output::OutputManagerState,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
    },
};
use std::{
    os::fd::AsRawFd,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

pub struct LoopData {
    state: AIGIState,
    display: Display<AIGIState>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setting up everyghin for the Wayland Compositor

    // Create the EventLoop
    //
    // In the EventLoop will be inserted notifiers that will trigger some
    // callbacks, the callbacks have as arguments:
    // + the notifier data
    // + the state of the EventLoop (LoopData in this case, composed by
    // the State of the compositor and the main object of the wayland protocol,
    // the Display (wl_display))
    // + and some Metadata (BOH)
    let mut event_loop: EventLoop<LoopData> = EventLoop::try_new()?;

    // Initialize the Backend and get all the important notifiers
    // that needs to be inserted in the event Loop
    //
    // Each notifier has a different functionality but before
    // insert those in the event_loop let's create the state and
    // then see how the notifiers interact with the State of the Compositor
    let (backend_data, notifiers) = BackendData::init()?;

    // Creation of the Wayand Display  (main objecet of the protocol)
    let mut display: Display<AIGIState> = Display::new()?;

    // Initialize the State of the compositor
    let mut aigi_state = AIGIState::init(event_loop.handle(), &mut display, backend_data)?;

    // Configure the server Socket
    let socket_notifier = ListeningSocketSource::new_auto()?;
    let socket_name = socket_notifier.socket_name().to_os_string();
    // Set the enviroment variable that Wayland clients can use.
    // They get the socket and connect to it.
    std::env::set_var("WAYLAND_DISPLAY", &socket_name);

    // Add the Display itself into the event loop to dispatch all the request
    let display_notifier = Generic::new(
        display.backend().poll_fd().as_raw_fd(),
        Interest::READ,
        Mode::Level,
    );

    // Let's create the Output Global
    let drm_surface = aigi_state.backend_data.device_data.gbm_surface.surface();
    let mode = drm_surface.current_mode();
    let wl_mode = output::Mode::from(mode);

    // Tells the client what the physical properties of the output are.
    // Create a new output which is an area in the compositor space
    // that can be used by clients.
    // Normally represents a monitor used by the compositor.
    //
    // TODO: understan why here is insered 0,0 and only then modified
    // why I can't diretly create it in the correct way?
    let output = output::Output::new(
        "monitor1".to_string(), // random name
        output::PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
    );
    // Clients can access the global objects to get the physical properties and output state.
    let _global = output.create_global::<AIGIState>(&display.handle());

    // last argoment (0,0) because it is mapped at the top right of the space
    output.change_current_state(Some(wl_mode), None, None, Some((0, 0).into()));
    output.set_preferred(wl_mode);

    // Set the output of a space with coordinates for the upper left corner of the surface.
    aigi_state.space.map_output(&output, (0, 0));

    // Let's create the Dmabuf Global
    let _global = aigi_state
        .dmabuf_state
        .create_global_with_default_feedback::<AIGIState>(
            &display.handle(),
            &aigi_state.dmabuf_default_feedback,
        );

    // Set up notifiers:

    // Add Wayland socket to event loop
    event_loop
        .handle()
        .insert_source(socket_notifier, |stream, _, state| {
            // Insert a new client into Display with data associated with that client.
            // This starts the management of the client, the communication is over the UnixStream.
            state
                .display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))
                .unwrap();
        })?;

    // Add the Display Notifier to manage all the Requests from the clients
    event_loop
        .handle()
        .insert_source(display_notifier, |_, _, state| {
            // Dispatch requests received from clients to callbacks for clients. The callbacks will
            // probably need to access the current compositor state, so that is passed along.
            state.display.dispatch_clients(&mut state.state).unwrap();
            // we must return a PostAction::Continue to tell the event loop to continue listening for events.
            Ok(PostAction::Continue)
        })?;

    // Add remaining notifiers

    // Session nofifier is NOT managed for now
    // event_loop.state
    event_loop
        .handle()
        .insert_source(notifiers.drm, |event, _, loop_data| match event {
            DrmEvent::VBlank(_crtc) => {
                render::frame_showed(&mut loop_data.state)
                    .expect("Something wrong happened during the rendering phase");
            }
            DrmEvent::Error(err) => {
                println!("An error occur in the DRM: {err}");
            }
        })?;

    // LibInput notifier, used to get Seat input and apply those input to the State
    event_loop
        .handle()
        .insert_source(notifiers.libinput, |event, _, loop_data| {
            handle_input(&mut loop_data.state, event);
        })?;

    // Insert timer in the loop
    event_loop.handle().insert_source(
        Timer::from_duration(Duration::from_secs(30)),
        |_, _, _| {
            panic!("Aborted");
        },
    )?;

    // initial rendering
    render::render_frame(&mut aigi_state)?;

    while aigi_state.running.load(Ordering::SeqCst) {
        let mut loop_data = LoopData {
            state: aigi_state,
            display,
        };
        let result = event_loop.dispatch(Some(Duration::from_millis(16)), &mut loop_data);
        LoopData {
            state: aigi_state,
            display,
        } = loop_data;

        if result.is_err() {
            aigi_state.running.store(false, Ordering::SeqCst);
        } else {
            aigi_state.space.refresh();
            //loop_data.state.popups.cleanup();
            display.flush_clients().unwrap();
        }
    }
    Ok(())
}
