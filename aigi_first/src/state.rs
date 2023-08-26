use super::tiling::{Split, TilingInfo};
use super::CalloopData;

use anyhow::{Error, Result};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::{layer_map_for_output, space::SpaceElement, Space, Window},
    input::{
        keyboard::{keysyms, FilterResult},
        pointer::CursorImageStatus,
        Seat, SeatHandler, SeatState,
    },
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, Mode, PostAction},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::ClientData,
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
            Client, Display, DisplayHandle,
        },
    },
    utils::{Logical, Point, Rectangle, Serial},
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

use std::{collections::HashMap, os::unix::prelude::AsRawFd, sync::Arc};

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState, // not sure about this
}

impl ClientData for ClientState {}

pub struct AIGIState {
    pub display_handle: DisplayHandle,

    pub space: Space<Window>,

    // Smithay State
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub seat: Seat<Self>,

    pub pointer_location: Point<f64, Logical>,
    pub cursor_status: CursorImageStatus,

    pub tiling_info: HashMap<WlSurface, TilingInfo>,
}

impl SeatHandler for AIGIState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    // for now do nothing here
    fn cursor_image(
        &mut self,
        _: &smithay::input::Seat<Self>,
        new_image: smithay::input::pointer::CursorImageStatus,
    ) {
        self.cursor_status = new_image;
    }

    fn focus_changed(&mut self, _: &smithay::input::Seat<Self>, _: Option<&WlSurface>) {}
}
delegate_seat!(AIGIState); // ??? BOH

impl CompositorHandler for AIGIState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    // Called on every buffer commit in Wayland to update a surface. This has the new state of the
    // surface.
    fn commit(&mut self, surface: &WlSurface) {
        // Let Smithay take the surface buffer so that desktop helpers get the new surface state.
        on_commit_buffer_handler::<Self>(surface);

        // Find the window with the xdg toplevel surface to update.
        match self
            .space
            .elements()
            .find(|w| w.toplevel().wl_surface() == surface)
            .cloned()
        {
            Some(window) => {
                // Refresh the window state.
                window.on_commit();

                // Find if the window has been configured yet.
                let initial_configure_sent = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .initial_configure_sent
                });

                if !initial_configure_sent {
                    // Configure window size/attributes.
                    window.toplevel().send_configure();
                }
            }
            None => (),
        }
    }
}
delegate_compositor!(AIGIState);

impl BufferHandler for AIGIState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for AIGIState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
delegate_shm!(AIGIState);

impl XdgShellHandler for AIGIState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let wl_surface = surface.wl_surface().clone();
        let window = Window::new(surface);
        //self.space.map_element(window, (0, 0), false);

        // How to implement a basic tiling window manager?
        // Just for each window associate a slit type
        //
        // when a new window needs to be spawned then the previously focused
        // will be used and splitted in the direction specified in the previous
        // window
        //
        // When it is canceled?
        // THEN there is a HUGE problem LOL (later I will think how to implement this)

        // Split Orintation of the previously focused window
        // None if there was now Window
        let focus_info: Option<(Window, TilingInfo)> = self
            .seat
            .get_keyboard()
            .unwrap()
            .current_focus()
            .and_then(|wl_surface| {
                Some((
                    self.space
                        .elements()
                        .find(|w| w.toplevel().wl_surface() == &wl_surface)
                        .cloned()
                        .expect("Impossible having a surface on focus not present in the Space"),
                    self.tiling_info
                        .get(&wl_surface)
                        .expect("Impossible surface not present in tiling info")
                        .clone(),
                ))
            });

        // Store all the new position of the modified windows

        let output = self.space.outputs().next();
        let output_geometry = output
            .and_then(|o| {
                let geo = dbg!(self.space.output_geometry(&o)?);
                let map = layer_map_for_output(&o);
                let zone = dbg!(map.non_exclusive_zone());
                Some(Rectangle::from_loc_and_size(geo.loc + zone.loc, zone.size))
            })
            .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), (800, 800)));
        println!("output_geometry: {output_geometry:?}");
        let mut new_positions: Vec<(Window, Point<i32, Logical>)> = vec![];

        let new_tiling_info = match focus_info {
            Some((
                focus_window,
                TilingInfo {
                    split: focus_split,
                    loc: focus_loc,
                },
            )) => {
                match focus_split {
                    // Always split on the right
                    Split::Horizontal => {
                        // in the focus window the only thing that needs to change is the
                        // width, it should be halfed
                        //
                        // the new window whould be positioned just at the same y
                        // with the same size of the previous and the only difference is that
                        // it should be positioned just after the end of the previous one

                        // test print output

                        let focus_geometry = focus_window.geometry();
                        println!("focus_gemotry: {focus_geometry:?}");

                        let height = focus_geometry.size.h;
                        let new_width = (focus_geometry.size.w as f32 / 2 as f32).floor() as i32;
                        println!("new_width: {new_width:?}");

                        focus_window
                            .toplevel()
                            .with_pending_state(|top_level_state| {
                                top_level_state.bounds = Some((new_width, height).into());
                                top_level_state.size = Some((new_width, height).into());
                                // here could be setted also the decoration mode
                            });
                        focus_window.toplevel().send_configure();

                        window.toplevel().with_pending_state(|top_level_state| {
                            top_level_state.bounds = Some((new_width, height).into());
                            top_level_state.size = Some(((new_width, height)).into());
                        });
                        // Do not send a configure here, the initial configure
                        // of a xdg_surface has to be sent during the commit if
                        // the surface is not already configured
                        //window.toplevel().send_configure();

                        let new_pos: Point<i32, Logical> =
                            (focus_loc.x + new_width, focus_loc.y).into();
                        new_positions.push((window, new_pos));
                        TilingInfo::new(focus_split, new_pos)
                    }
                    Split::Vertical => {
                        let focus_geometry = focus_window.geometry();
                        println!("focus_gemotry: {focus_geometry:?}");

                        let width = focus_geometry.size.w;
                        let new_height = (focus_geometry.size.h as f32 / 2 as f32).floor() as i32;
                        println!("new_height: {new_height:?}");

                        focus_window
                            .toplevel()
                            .with_pending_state(|top_level_state| {
                                top_level_state.bounds = Some((width, new_height).into());
                                top_level_state.size = Some((width, new_height).into());
                                // here could be setted also the decoration mode
                            });
                        focus_window.toplevel().send_configure();

                        window.toplevel().with_pending_state(|top_level_state| {
                            top_level_state.bounds = Some((width, new_height).into());
                            top_level_state.size = Some(((width, new_height)).into());
                        });
                        // Do not send a configure here, the initial configure
                        // of a xdg_surface has to be sent during the commit if
                        // the surface is not already configured
                        //window.toplevel().send_configure();

                        let new_pos: Point<i32, Logical> =
                            (focus_loc.x, focus_loc.y + new_height).into();
                        new_positions.push((window, new_pos));
                        TilingInfo::new(focus_split, new_pos)
                    }
                }
            }
            None => {
                // rendere full size screen
                // and insert with default orientation
                let output = self.space.outputs().next();
                let output_geometry = output
                    .and_then(|o| {
                        let geo = dbg!(self.space.output_geometry(&o)?);
                        let map = layer_map_for_output(&o);
                        let zone = dbg!(map.non_exclusive_zone());
                        Some(Rectangle::from_loc_and_size(geo.loc + zone.loc, zone.size))
                    })
                    .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), (800, 800)));

                let (output_x, output_y) = output_geometry.size.into();

                window.toplevel().with_pending_state(|top_level_state| {
                    top_level_state.bounds = Some((output_x, output_y).into());
                    top_level_state.size = Some((output_x, output_y).into());
                });
                // Do not send a configure here, the initial configure
                // of a xdg_surface has to be sent during the commit if
                // the surface is not already configured
                //window.toplevel().send_configure();
                new_positions.push((window, (0, 0).into()));
                TilingInfo::default()
            }
        };

        // Insert new surface in tiling window info map
        self.tiling_info.insert(wl_surface, new_tiling_info);

        // Update position of mofidied windows
        for (win, new_pos) in new_positions.into_iter() {
            self.space.map_element(win, new_pos, true);
        }

        // test split horizontally

        // alternative:
        //let (output_x, output_y) = self.space.output_geometry(output).unwrap() (.size()?);

        // println!("output x: {output_x} y: {output_y}");

        //let windows: Vec<Window> = self.space.elements().cloned().collect();
        //let window_x = output_x;
        //let window_y = output_y / windows.len() as i32;

        //let mut new_positions: Vec<(Window, (i32, i32))> = vec![];

        //for (index, window) in windows.into_iter().enumerate() {
        //    let top_level = window.toplevel();
        //    top_level.with_pending_state(|top_level_state| {
        //        top_level_state.size = Some((window_x, window_y).into());
        //        // here could be setted also the decoration mode
        //    });
        //    top_level.send_configure();
        //    new_positions.push((window, (0, window_y * index as i32)));
        //}

        //for (win, new_pos) in new_positions.into_iter() {
        //    self.space.map_element(win, new_pos, false);
        //}

        // self.update_tiling();
    }

    fn new_popup(&mut self, _: PopupSurface, _: PositionerState) {}

    fn move_request(&mut self, _: ToplevelSurface, _: wl_seat::WlSeat, _: Serial) {}

    fn resize_request(
        &mut self,
        _: ToplevelSurface,
        _: wl_seat::WlSeat,
        _: Serial,
        _: xdg_toplevel::ResizeEdge,
    ) {
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
}
delegate_xdg_shell!(AIGIState);

delegate_output!(AIGIState);

impl ClientDndGrabHandler for AIGIState {}
impl ServerDndGrabHandler for AIGIState {}

impl DataDeviceHandler for AIGIState {
    type SelectionUserData = ();
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
delegate_data_device!(AIGIState);

impl AIGIState {
    pub fn new(
        event_loop: &mut EventLoop<CalloopData>,
        display: &mut Display<Self>,
    ) -> Result<Self, Error> {
        // Configure all the required Globals
        let dh = display.handle();

        // The compositor for our compositor.
        let compositor_state = CompositorState::new::<AIGIState>(&dh);
        // Shared memory buffer for sharing buffers with clients. For example wl_buffer uses wl_shm
        // to create a shared buffer for the compositor to access the surface contents of the client.
        let shm_state = ShmState::new::<AIGIState>(&dh, vec![]);
        // An output is an area of space that the compositor uses, the OutputManagerState tells
        // wl_output to use the xdg-output extension.
        let output_manager_state = OutputManagerState::new_with_xdg_output::<AIGIState>(&dh);
        // Used for desktop applications, defines two types of Wayland surfaces clients can use,
        // "toplevel" (for the main application area) and "popup" (for dialogs/tooltips/etc).
        let xdg_shell_state = XdgShellState::new::<AIGIState>(&dh);
        // A space to map windows on. Keeps track of windows and outputs, can access either with
        // space.elements() and space.outputs().
        let space = Space::<Window>::default();
        // Manage copy/paste and drag-and-drop from inputs.
        let data_device_state = DataDeviceState::new::<AIGIState>(&dh);

        // A seat is a group of input devices like keyboards, pointers, etc. This manages the seat
        // state.
        let mut seat_state = SeatState::<AIGIState>::new();
        // Create a new seat from the seat state, we pass in a name .
        let mut seat: Seat<AIGIState> = seat_state.new_wl_seat(&dh, "mwm_seat");

        // Add a keyboard with repeat rate and delay in milliseconds. The repeat is the time to
        // repeat, then delay is how long to wait until the next repeat.
        seat.add_keyboard(Default::default(), 500, 500)?;
        // Add pointer to seat.
        seat.add_pointer();

        Ok(AIGIState {
            display_handle: dh,
            space,
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            seat,
            pointer_location: (0.0, 0.0).into(),
            cursor_status: CursorImageStatus::Default,
            tiling_info: HashMap::new(),
        })
    }

    fn update_tiling(&mut self) {}
}
