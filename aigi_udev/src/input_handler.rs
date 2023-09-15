use smithay::{
    backend::{
        input::{AbsolutePositionEvent, Event, InputEvent, KeyState, KeyboardKeyEvent},
        libinput::LibinputInputBackend,
    },
    input::keyboard::{keysyms, FilterResult},
    utils::SERIAL_COUNTER,
};

use crate::{state::AIGIState, tiling};

pub enum Action {
    exec_process(&'static str),
    change_split(tiling::Split),
}

// This function based on the input will apply all the required
// side effects to the AIGIState and return a Action that the AIGIState
// should take actively
pub fn handle_input(state: &mut AIGIState, event: InputEvent<LibinputInputBackend>) {
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
                    if press_state == KeyState::Pressed && keysym.modified_sym() == keysyms::KEY_W {
                        FilterResult::Intercept(Action::exec_process("weston-terminal"))
                    } else if press_state == KeyState::Pressed
                        && keysym.modified_sym() == keysyms::KEY_A
                    {
                        FilterResult::Intercept(Action::exec_process("alacritty"))
                    } else if press_state == KeyState::Pressed
                        && keysym.modified_sym() == keysyms::KEY_V
                    {
                        FilterResult::Intercept(Action::change_split(tiling::Split::Vertical))
                    } else if press_state == KeyState::Pressed
                        && keysym.modified_sym() == keysyms::KEY_O
                    {
                        FilterResult::Intercept(Action::change_split(tiling::Split::Horizontal))
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
                            state.tiling_state.set_split(&wl_surface, new_split);
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
            let surface_under_pointer =
                state
                    .space
                    .element_under(pointer_location)
                    .and_then(|(window, location)| {
                        window
                            .surface_under(
                                pointer_location - location.to_f64(),
                                smithay::desktop::WindowSurfaceType::ALL,
                            )
                            .map(|(s, p)| (s, p + location))
                    });

            let mut serial = SERIAL_COUNTER.next_serial();
            state.seat.get_keyboard().unwrap().set_focus(
                state,
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
                &smithay::input::pointer::MotionEvent {
                    location: pointer_location,
                    serial,
                    time: event.time_msec(),
                },
            );
        }
        _ => (),
    }
}
