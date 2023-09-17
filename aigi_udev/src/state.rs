use crate::backend::BackendData;

use super::tiling::{Split, TilingState};
use super::LoopData;

use anyhow::{Error, Result};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::{ImportDma, ImportMemWl};
use smithay::delegate_dmabuf;
use smithay::output::Output;
use smithay::reexports::calloop::LoopHandle;
use smithay::wayland::dmabuf::{
    DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportError,
};
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
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

use std::sync::atomic::AtomicBool;
use std::{collections::HashMap, os::unix::prelude::AsRawFd, sync::Arc};

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState, // not sure about this
}

impl ClientData for ClientState {}

pub struct AIGIState {
    // everythin related with the backend
    pub backend_data: BackendData,

    // main wayland object
    pub display_handle: DisplayHandle,

    // loop handle
    pub handle: LoopHandle<'static, LoopData>,

    // Atomic bool to keeps track of the running compositor
    pub running: AtomicBool,

    // desktop stuff
    pub space: Space<Window>,

    // Smithay State
    pub compositor_state: CompositorState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Self>,
    pub shm_state: ShmState,
    pub xdg_shell_state: XdgShellState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_default_feedback: DmabufFeedback,

    // input things
    pub seat: Seat<Self>,
    pub pointer_location: Point<f64, Logical>,
    pub cursor_status: CursorImageStatus,

    // tiling state
    pub tiling_state: TilingState,
}

impl CompositorHandler for AIGIState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    // Called on every buffer commit in Wayland to update a surface
    //
    // With Events and Requests between client and server a pending state is defined
    // and then on commit the pending state becomes the current state
    //
    // There are two main types of wl_surface, synchronized and NOT,
    // the synchronized apply effectively the current stata only when the parent commit it
    // (it works recursively), while if the surface is not syncronized it is directly applied
    fn commit(&mut self, surface: &WlSurface) {
        // Let Smithay take the surface buffer so that desktop helpers get the new surface state.
        on_commit_buffer_handler::<Self>(surface);

        // Should be done something on the gpu_managed called `early_import`

        // Now we should AVOID update the state of a surface if it is
        // sync (see anvil impmentation of this method) but the first version
        // of aigi will NOT manage popus or subsurfaces in general
        // so ONLY top_level surfaces will commit thins and no check will be done before!

        // Find the window with the xdg toplevel surface to update.
        if let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().wl_surface() == surface)
            .cloned()
        {
            // Refresh the window state.
            window.on_commit();

            // Ensure Initial Configuration
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

            //
            // Should be also managed some Initial cofiguration on the layer_map
            // (see ensure_initial_configuration in anvil/src/shell/mod)
        }

        // commit of the popup should now be managed
    }
}
delegate_compositor!(AIGIState);

delegate_output!(AIGIState);

impl SeatHandler for AIGIState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(
        &mut self,
        _: &smithay::input::Seat<Self>,
        new_image: smithay::input::pointer::CursorImageStatus,
    ) {
        // Change the cursor image to respect what defined by the client
        self.cursor_status = new_image;
    }

    // do nothing for now, here should be inserted all the side effects
    // of changing the focus
    fn focus_changed(&mut self, _: &smithay::input::Seat<Self>, _: Option<&WlSurface>) {}
}
delegate_seat!(AIGIState);

// Even inside Anvil is not implemented
// not sure if we will ever need to update things when a buffer is destroyed
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
        let window = Window::new(surface);

        // get the window underfocus
        let focus_window: Option<Window> = self
            .seat
            .get_keyboard()
            .unwrap()
            .current_focus()
            .and_then(|wl_surface| {
                Some(
                    self.space
                        .elements()
                        .find(|w| w.toplevel().wl_surface() == &wl_surface)
                        .cloned()
                        .expect("Impossible having a surface on focus not present in the Space"),
                )
            });

        let node_to_update = match focus_window {
            Some(focus_window) => self.tiling_state.split(focus_window, window),
            None => {
                // render full size screen
                // TODO: in the state should be added something like output geometry
                // to not fetch it every time
                let output = self.space.outputs().next();
                let output_geometry = output
                    .and_then(|o| {
                        let geo = self.space.output_geometry(&o)?;
                        let map = layer_map_for_output(&o);
                        let zone = map.non_exclusive_zone();
                        Some(Rectangle::from_loc_and_size(geo.loc + zone.loc, zone.size))
                    })
                    .unwrap_or_else(|| Rectangle::from_loc_and_size((0, 0), (800, 800)));

                // Do not send a configure here, the initial configure
                // of a xdg_surface has to be sent during the commit if
                // the surface is not already configured
                // window.toplevel().send_configure();

                self.tiling_state
                    .insert_head(window, output_geometry)
                    .unwrap()
            }
        };

        self.tiling_state
            .update_space(node_to_update, &mut self.space);
    }

    fn new_popup(&mut self, _: PopupSurface, _: PositionerState) {}

    // TODO
    fn move_request(&mut self, _: ToplevelSurface, _: wl_seat::WlSeat, _: Serial) {}

    // TODO
    fn resize_request(
        &mut self,
        _: ToplevelSurface,
        _: wl_seat::WlSeat,
        _: Serial,
        _: xdg_toplevel::ResizeEdge,
    ) {
    }

    // TODO
    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let window = self
            .space
            .elements()
            .find(|w| *w.toplevel() == surface)
            .expect("IMP destroy a non existring surface")
            .clone();
        self.space.unmap_elem(&window);

        // TODO remove this unwrap :sweat_smile:
        if let Some(node_to_update) = self.tiling_state.destroy(surface.wl_surface()).unwrap() {
            self.tiling_state
                .update_space(node_to_update, &mut self.space);
        }
    }
}
delegate_xdg_shell!(AIGIState);

impl DmabufHandler for AIGIState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
    ) -> Result<(), ImportError> {
        self.backend_data
            .gpu_manager
            .single_renderer(&self.backend_data.device_data.render_node)
            .and_then(|mut renderer| renderer.import_dmabuf(&dmabuf, None))
            .map(|_| ())
            .map_err(|_| ImportError::Failed)
    }
}
delegate_dmabuf!(AIGIState);

impl AIGIState {
    pub fn init(
        even_loop_handle: LoopHandle<'static, LoopData>,
        display: &mut Display<Self>,
        mut backend_data: BackendData,
    ) -> Result<Self, Error> {
        // Things to be done:
        // + Clock?
        // + Init the wayland socket to accept connections by the clients
        // + Init all the Globals
        // + Init the input
        //
        // -> BufferHandler
        //
        // Which are the globals:
        // + CompositorState
        //   + it stores in a coherent way the state of surface trees with subsurfaces,
        //   it require to implement the CompositorHandler where the `commit` method
        //   is called when there is an agreement between the client and server about the state
        //   of the surface (pending_state -> current_state)
        //
        //   + Usage:
        //      - delegate_compositor!
        //      - CompositorHandler
        // + DataDeviceState
        //   + The data device is wayland’s abstraction to represent both selection (copy/paste) and drag’n’drop actions
        //
        //   + Usage
        //      - implementation of the DataDeviceHandler
        //      - implementation of ClientDndGrabHandler (used to update dnd icon)
        //      - implementation of ServerDndGrabHandler
        //      - delegate_data_device!
        // + WlrLayerShellState (?)
        //   + Utilities for handling shell surfaces with the wlr_layer_shell protocol,
        //     this state allows you to retrieve a list of surfaces currently known to the shell global.
        //
        //   + xdg-shell is for regular windows, wlr-layer-shell is for UI components that you'd think as
        //     "part of the desktop environment", such as a taskbar, desktop widgets, screenshotting overlays,
        //     animated background images, etc...
        //
        //   + Usage:
        //      - impl WlrLayerShellHandler
        //      - delegate_layer_shell!
        // + OutputManagerState
        //   + This object is on top of smithay::output::Output to provide additinal functionalities.
        //     Firstly you need to create an smithay::output::Output object and then Output::createGlobal
        //     to create a notifier in the event_loop to advertise a new output global to clients (how can be already insered in the event loop?)
        //
        //     This state can be summarized as the glue between the real Output object (with all the settings and stuff) and the clients
        //
        //   + Usage:
        //      - delegate_output!
        // + PrimarySelectionState
        //   + This primary selection is a shortcut to the common clipboard selection,
        //     where text just needs to be selected in order to allow copying it elsewhere
        //     The de facto way to perform this action is the middle mouse button,
        //     although it is not limited to this one.
        //
        //   + Usage:
        //      - implementation of PrimarySelectionHandler
        //      - delegate_primary_selection!
        // + SeatState
        //   + Input abstractions, delegate type for all Seat globals,
        //     events will be forwarded to an instance of the Seat global.
        //
        //   + Usage:
        //      - implementation of SeatHandler, can be added logic of focus changed and
        //      to cursor_image changed
        //      - delegate_seat!
        //
        // + ShmState
        //   + SHM (Shared Memory) is the most basic way wayland clients can send content to the compositor:
        //     by sending a file descriptor to some (likely RAM-backed) storage containing the actual data.
        //     The ShmState let you creat the ShmGlobal and then manage it
        //
        //   + Usage:
        //      - ShmHandler
        //      - delegate_shm!
        // + ViewporterState (?)
        //   + This extended interface will then allow cropping and scaling the surface contents,
        //     effectively disconnecting the direct relationship between the buffer and the surface size
        //
        //   + Usage:
        //      - delegate_viewporter!
        // + XdgActivationState (?)
        //   + Utilities for handling activation requests with the xdg_activation protocol
        //
        //   + Usage:
        //      - implementation XdgActivationHandler
        //      - delegate_xdg_activation!
        // + XdgDecorationState (?)
        //   + XDG Window decoration manager
        //     This interface allows a compositor to announce support for server-side decorations.
        //     A client can use this protocol to request being decorated by a supporting compositor.
        //
        //   + Usage:
        //      - impl XdgDecorationHandler
        //      - Delegate_xdg_decoration!
        // + XdgShellState
        //   + This implementation can track for you the various shell surfaces
        //     defined by the clients by handling the xdg_shell protocol.
        //     It allows you to easily access a list of all shell surfaces
        //     defined by your clients access their associated metadata
        //     and underlying wl_surfaces.
        //
        //   + Usage:
        //      - impl XdgShellHandler
        //      - delegate_xdg_shell!
        //
        // + PresentationState (?)
        //   + Utilities for handling the wp_presentation protocol

        //   + Usage:
        //      - delegate_presentation!
        // + FractionalScaleManagerState (?)
        //   + Utilities for handling the wp_fractional_scale protocol
        //
        //   + Usage:
        //      - impl FractionalScaleHandler
        //      - delegate_fractional_scale!
        // + TextInputManagerState (?)
        //   + Utilities for text input support
        //     This module provides you with utilities to handle text input surfaces,
        //     it is usually used in conjunction with the input method module.
        //
        //   + Usage:
        //      - delegate_text_input_manager!
        // + InputMethodManagerState (?)
        //   + Utilities for input method support
        //     This module provides you with utilities to handle input methods, it must be used in conjunction with the text input module to work.
        //
        //   + Usage:
        //      - delegate_input_method_manager!
        // + VirtualKeyboardManagerState (?)
        //   + This module provides you with utilities to handle virtual keyboard instances. It can be used standalone to
        //     implement virtual keyboards or together with an input method to pass through keys from the keyboard.
        //
        //   + Usage:
        //      - delegate_virtual_keyboard_manager!
        // + RelativePointerManagerState
        //   + Utilities for relative pointer support
        //
        //   + Usage:
        //      - delegate_relative_pointer!
        // + PointerGesturesState
        //   + Utilities for pointer gestures support
        //     This protocol allows clients to receive touchpad gestures
        //
        //   + Usage:
        //      - delegate_pointer_gestures!
        // + SecurityContextState (?)
        //   + Utilities for handling the security context protocol
        //
        //   + Usage:
        //      - impl SecurityContextHandler
        //      - delegate_security_context!
        // + DmabufState
        //   + Delegate type for all dmabuf globals.
        //
        //   + Usage:
        //      - DmabufHandler
        //      - delegate_dmabuf!

        // Configure all the required Globals
        let dh = display.handle();

        // Extract Renderer from the backend to later use it
        // to extract all the informatin needed to initialize
        // the AigiState
        let render_node = &backend_data.device_data.render_node;
        let renderer = backend_data
            .gpu_manager
            .single_renderer(render_node)
            .expect("Impossible get Renderer");

        // The compositor for our compositor.
        let compositor_state = CompositorState::new::<AIGIState>(&dh);
        // Shared memory buffer for sharing buffers with clients. For example wl_buffer uses wl_shm
        // to create a shared buffer for the compositor to access the surface contents of the client.
        let mut shm_state = ShmState::new::<AIGIState>(&dh, vec![]);
        shm_state.update_formats(renderer.shm_formats());

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
        // let data_device_state = DataDeviceState::new::<AIGIState>(&dh);

        // A seat is a group of input devices like keyboards, pointers, etc. This manages the seat
        // state.
        let mut seat_state = SeatState::<AIGIState>::new();
        // Create a new seat from the seat state, we pass in a name .
        let mut seat: Seat<AIGIState> = seat_state.new_wl_seat(&dh, "aigi_seat");

        // Add a keyboard with repeat rate and delay in milliseconds. The repeat is the time to
        // repeat, then delay is how long to wait until the next repeat.
        seat.add_keyboard(Default::default(), 500, 500)?;
        // Add pointer to seat.
        seat.add_pointer();

        // DO NOT CARE ABOUT egl hardware acceleration
        // because it's the mechanism used by mesa internally before the
        // linux-dmabuf protocol was created and standartized

        // init dmabuf default feeback with what our device supports
        let dmabuf_formats = renderer.dmabuf_formats().collect::<Vec<_>>();
        let dmabuf_default_feedback =
            DmabufFeedbackBuilder::new(render_node.dev_id(), dmabuf_formats)
                .build()
                .unwrap();
        let dmabuf_state = DmabufState::new();

        // TODO: the creation of globals should not be in the
        // initialization of the state!!
        //let global = dmabuf_state
        // .create_global_with_default_feedback::<AIGIState>(&display.handle(), &default_feedback);

        // TODO: inside anvil there is a really weird uage of the Default Feedback that
        // is part of the rendering part... I can't understand it for now so I will go deeper
        // later... hope the global with the default feedback is enough for now

        let tiling_state = TilingState::init();

        Ok(AIGIState {
            display_handle: dh,
            handle: even_loop_handle,
            space,
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            //data_device_state,
            seat,
            pointer_location: (0.0, 0.0).into(),
            cursor_status: CursorImageStatus::Default,
            tiling_state,
            running: AtomicBool::new(true),
            backend_data,
            dmabuf_default_feedback,
            dmabuf_state,
        })
    }

    pub fn get_output(&mut self) -> Result<&Output, Box<dyn std::error::Error>> {
        self.space
            .outputs()
            .next()
            .ok_or("No output available".into())
    }
}
