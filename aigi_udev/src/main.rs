mod backend;
mod pointer;
mod state;
mod tiling;

use backend::BackendData;
use pointer::{PointerElement, PointerRenderElement};
use state::{AIGIState, ClientState};

use anyhow::{Error, Result};
use smithay::{
    backend::{
        input::{AbsolutePositionEvent, Event, InputEvent, KeyState, KeyboardKeyEvent},
        renderer::{
            damage::OutputDamageTracker,
            element::{surface::WaylandSurfaceRenderElement, AsRenderElements},
            gles::{GlesRenderer, GlesTexture},
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
use std::{os::fd::AsRawFd, sync::Arc};

pub struct LoopData {
    state: AIGIState,
    display: Display<AIGIState>,
}

pub enum Action {
    exec_process(&'static str),
    change_split(tiling::Split),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setting up everyghin for the Wayland Compositor

    // Create the EventLoop
    let mut event_loop: EventLoop<LoopData> = EventLoop::try_new()?;

    // Create the Wayand Display  (main objecet)
    let mut display: Display<AIGIState> = Display::new()?;

    let mut backend_data = BackendData::init(&mut event_loop /* , &mut display*/);

    // Create the Initial State of the composito
    let mut aigi_state = AIGIState::new(&mut event_loop, &mut display)?;

    // Configure the server Socket
    let socket = ListeningSocketSource::new_auto()?;
    let socket_name = socket.socket_name().to_os_string();

    // Add Wayland socket to event loop
    event_loop
        .handle()
        .insert_source(socket, |stream, _, state| {
            // Insert a new client into Display with data associated with that client.
            // This starts the management of the client, the communication is over the UnixStream.
            state
                .display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))
                .unwrap();
        })?;

    // Add the Display itself into the event loop to dispatch all the request
    event_loop.handle().insert_source(
        Generic::new(
            display.backend().poll_fd().as_raw_fd(),
            Interest::READ,
            Mode::Level,
        ),
        |_, _, state| {
            // Dispatch requests received from clients to callbacks for clients. The callbacks will
            // probably need to access the current compositor state, so that is passed along.
            state.display.dispatch_clients(&mut state.state).unwrap();
            // we must return a PostAction::Continue to tell the event loop to continue listening for events.
            Ok(PostAction::Continue)
        },
    )?;

    let (mut backend, mut winit) = winit::init()?;

    let mode = output::Mode {
        size: backend.window_size().physical_size,
        refresh: 60_000,
    };

    // Tells the client what the physical properties of the output are.
    // Create a new output which is an area in the compositor space that can be used by clients.
    // Normally represents a monitor used by the compositor.
    let output = output::Output::new(
        "winit".to_string(),
        output::PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
        },
    );
    // Clients can access the global objects to get the physical properties and output state.
    let _global = output.create_global::<AIGIState>(&display.handle());
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    // Set the output of a space with coordinates for the upper left corner of the surface.
    aigi_state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    // Set the enviroment variable that Wayland clients can use. They get the socket and connect to
    // it.
    std::env::set_var("WAYLAND_DISPLAY", &socket_name);

    let start_time = std::time::Instant::now();
    let timer = Timer::immediate();

    // An element that renders the pointer when rendering the output to display.
    let mut pointer_element = PointerElement::<GlesTexture>::default();

    event_loop
        .handle()
        .insert_source(timer, move |_, _, data| {
            let display = &mut data.display;
            let mut state = &mut data.state;

            // Process events from winit event loop
            winit
                .dispatch_new_events(|event| match event {
                    WinitEvent::Resized { size, .. } => {
                        output.change_current_state(
                            Some(output::Mode {
                                size,
                                refresh: 60_000,
                            }),
                            None,
                            None,
                            None,
                        );
                        layer_map_for_output(&output).arrange();
                    }
                    WinitEvent::Input(event) => {
                        match event {
                            InputEvent::Keyboard { event } => {
                                // If we received a keyboard event, get the keyboard from the seat
                                // and process a key input.
                                let serial = SERIAL_COUNTER.next_serial();
                                let time = Event::time_msec(&event);
                                let press_state = event.state();
                                let action = state.seat.get_keyboard().unwrap().input::<Action, _>(
                                    state,
                                    event.key_code(),
                                    press_state,
                                    serial,
                                    time,
                                    |_, _, keysym| {
                                        // If the user pressed the letter T, return the action value of
                                        // 1.
                                        if press_state == KeyState::Pressed
                                            && keysym.modified_sym() == keysyms::KEY_W
                                        {
                                            FilterResult::Intercept(Action::exec_process(
                                                "weston-terminal",
                                            ))
                                        } else if press_state == KeyState::Pressed
                                            && keysym.modified_sym() == keysyms::KEY_A
                                        {
                                            FilterResult::Intercept(Action::exec_process(
                                                "alacritty",
                                            ))
                                        } else if press_state == KeyState::Pressed
                                            && keysym.modified_sym() == keysyms::KEY_V
                                        {
                                            FilterResult::Intercept(Action::change_split(
                                                tiling::Split::Vertical,
                                            ))
                                        } else if press_state == KeyState::Pressed
                                            && keysym.modified_sym() == keysyms::KEY_O
                                        {
                                            FilterResult::Intercept(Action::change_split(
                                                tiling::Split::Horizontal,
                                            ))
                                        } else {
                                            FilterResult::Forward
                                        }
                                    },
                                );

                                match action {
                                    Some(Action::exec_process(process_name)) => {
                                        std::process::Command::new(process_name).spawn().unwrap();
                                    }
                                    Some(Action::change_split(new_split)) => {
                                        match state.seat.get_keyboard().unwrap().current_focus() {
                                            Some(wl_surface) => {
                                                //state.tiling_info.get_mut(&wl_surface).expect("Impossible havinfg a window not present in tiling info").split = new_split;
                                                state
                                                    .tiling_state
                                                    .set_split(&wl_surface, new_split);
                                            }
                                            None => (),
                                        }
                                    }
                                    _ => (),
                                }
                            }
                            InputEvent::PointerMotionAbsolute { event, .. } => {
                                // Get the first output.
                                let output = state.space.outputs().next().unwrap();
                                let output_geo = state.space.output_geometry(output).unwrap();
                                // Convert the device position to use the output coordinate system.
                                let pointer_location = event.position_transformed(output_geo.size);

                                state.pointer_location = pointer_location;

                                //println!("Pointer Location: {pointer_location:?}");

                                let pointer = state.seat.get_pointer().unwrap();

                                // Get the surface below the pointer if it exists. First get the
                                // element under a position, then get the surface under that position.
                                let surface_under_pointer = state
                                    .space
                                    .element_under(pointer_location)
                                    .and_then(|(window, location)| {
                                        window
                                            .surface_under(
                                                pointer_location - location.to_f64(),
                                                WindowSurfaceType::ALL,
                                            )
                                            .map(|(s, p)| (s, p + location))
                                    });

                                let mut serial = SERIAL_COUNTER.next_serial();
                                state.seat.get_keyboard().unwrap().set_focus(
                                    &mut state,
                                    surface_under_pointer
                                        .as_ref()
                                        .and_then(|s| Some(s.0.clone())),
                                    serial,
                                );

                                serial = SERIAL_COUNTER.next_serial();

                                // Send the motion event to the client.
                                pointer.motion(
                                    state,
                                    surface_under_pointer,
                                    &MotionEvent {
                                        location: pointer_location,
                                        serial,
                                        time: event.time_msec(),
                                    },
                                );
                            }
                            _ => (),
                        }
                    }
                    _ => (),
                })
                .unwrap();

            backend.bind().unwrap();

            // Update the pointer element with the clock to determine which xcursor image to show,
            // and the cursor status. The status can be set to a surface by a window to show a
            // custom cursor set by the window.
            //pointer_element.set_current_delay(&state.clock);
            pointer_element.set_status(state.cursor_status.clone());

            // Get the cursor position if the output is fractionally scaled.
            let scale = Scale::from(output.current_scale().fractional_scale());
            let cursor_pos = state.pointer_location;
            let cursor_pos_scaled = cursor_pos.to_physical(scale).to_i32_round();

            // Get the rendered elements from the pointer element.
            let elements = pointer_element.render_elements::<PointerRenderElement<GlesRenderer>>(
                backend.renderer(),
                cursor_pos_scaled,
                scale,
                1.0,
            );

            // Render output by providing backend renderer, the output, the space, and the
            // damage_tracked_renderer for tracking where the surface is damaged.
            // TODO: Implement damage tracking.
            smithay::desktop::space::render_output::<_, PointerRenderElement<GlesRenderer>, _, _>(
                &output,
                backend.renderer(),
                1.0,
                0,
                [&state.space],
                elements.as_slice(),
                &mut damage_tracker,
                [0.1, 0.1, 0.1, 1.0],
            )
            .unwrap();

            // Submit the back buffer to the display.
            backend.submit(None).unwrap();

            // For each of the windows send the frame callbacks to windows telling them to draw.
            state.space.elements().for_each(|window| {
                window.send_frame(
                    &output,
                    start_time.elapsed(),
                    Some(core::time::Duration::ZERO),
                    |_, _| Some(output.clone()),
                )
            });

            // Refresh space state and handle certain events like enter/leave for outputs/windows.
            state.space.refresh();

            // Flush the outgoing buffers containing events so the clients get them.
            display.flush_clients().unwrap();

            // Wait 16 milliseconds until next event.
            TimeoutAction::ToDuration(core::time::Duration::from_millis(16))
        })
        .unwrap();

    let mut data = LoopData {
        state: aigi_state,
        display,
    };

    // Run the event loop
    event_loop.run(None, &mut data, |_| {})?;

    Ok(())
}
