use std::time::Duration;

use smithay::{
    backend::{
        allocator::gbm::GbmAllocator,
        drm::{DrmDeviceFd, GbmBufferedSurface},
        renderer::{
            damage::OutputDamageTracker,
            element::AsRenderElements,
            gles::{GlesRenderer, GlesTexture},
            multigpu::{gbm::GbmGlesBackend, MultiRenderer, MultiTexture},
            Bind, ImportAll, ImportMem,
        },
    },
    desktop::{space::SpaceRenderElements, Space, Window},
    input::{pointer::CursorImageStatus, SeatHandler},
    output::Output,
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Point, Scale},
};

use crate::{
    pointer::{PointerElement, PointerRenderElement},
    state::AIGIState,
};

type UdevRenderer<'a, 'b> =
    MultiRenderer<'a, 'a, 'b, GbmGlesBackend<GlesRenderer>, GbmGlesBackend<GlesRenderer>>; // size = 112 (0x70), align = 0x8

smithay::backend::renderer::element::render_elements! {
    pub OutputRenderElements<R, E> where R: ImportAll + ImportMem;
    Space=SpaceRenderElements<R, E>,
    Pointer=PointerRenderElement<R>,
}

pub fn frame_showed(state: &mut AIGIState) -> Result<(), Box<dyn std::error::Error>> {
    // Define the previous frame as correctly submitted

    state
        .backend_data
        .device_data
        .gbm_surface
        .frame_submitted()?;

    // Here should be created a time to let the clients render their frames
    let timer = match state.get_output()?.current_mode() {
        Some(mode) => Timer::from_duration(Duration::from_millis(
            ((1_000_000f32 / mode.refresh as f32) * 0.6f32) as u64,
        )),
        None => return Err("Mode not setted in the output".into()),
    };

    state
        .handle
        .insert_source(timer, |_, _, loop_data| {
            render_frame(&mut loop_data.state).unwrap();
            TimeoutAction::Drop
        })
        .expect("failed to schedule frame timer");

    Ok(())
}

pub fn render_frame<'state, 'a, 'b>(
    state: &'state mut AIGIState,
    // gbm_surface: &mut GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>,
    // output: &Output,
    // renderer: &mut UdevRenderer<'a, 'b>,
    // cursor_status: CursorImageStatus,
    // pointer_location: Point<f64, Logical>,
    // space: &Space<Window>,
) -> Result<(), Box<dyn std::error::Error>>
where
    'state: 'a + 'b,
    'b: 'a,
{
    let gbm_surface = &mut state.backend_data.device_data.gbm_surface;
    //let output = state.get_output()?;
    let output = state.space.outputs().next().unwrap();
    let mut renderer = state
        .backend_data
        .gpu_manager
        .single_renderer(&state.backend_data.device_data.render_node)
        .map_err(|_| "Impossible extract Renderer from State")?;

    // NOW LET'S PREPARE ALL THE ELEMENTS
    // only two sets for now, the cursor image and the one present in the Space

    // An element that renders the pointer when rendering the output to display.
    let mut pointer_element = PointerElement::<MultiTexture>::new(&mut renderer);

    // Update the pointer element with the clock to determine which xcursor image to show,
    // and the cursor status. The status can be set to a surface by a window to show a
    // custom cursor set by the window.
    //pointer_element.set_current_delay(&state.clock);
    pointer_element.set_status(state.cursor_status.clone());

    // Get the cursor position if the output is fractionally scaled.
    let scale = Scale::from(output.current_scale().fractional_scale());
    //let cursor_pos = pointer_location;
    //let cursor_pos_scaled = cursor_pos.to_physical(scale).to_i32_round();

    // println!("cursor pos: {:?}", cursor_pos);

    // Get the rendered elements from the pointer element.
    let custom_elements = pointer_element
        .render_elements::<PointerRenderElement<UdevRenderer<'a, 'b>>>(
            &mut renderer,
            //cursor_pos_scaled,
            state.pointer_location.to_physical(1.0).to_i32_round(),
            scale,
            1.0,
        );

    println!("custom elements len: {}", custom_elements.len());

    let (dmabuf, age) = gbm_surface.next_buffer()?;
    renderer.bind(dmabuf)?;

    // insered just because I can't do without
    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    smithay::desktop::space::render_output::<_, PointerRenderElement<UdevRenderer<'a, 'b>>, _, _>(
        &output,
        &mut renderer,
        1.0,
        0,
        [&state.space],
        custom_elements.as_slice(),
        &mut damage_tracker,
        [0.1, 0.1, 0.1, 1.0],
    )
    .map_err(|_| "Impossible render Space")?;

    gbm_surface.queue_buffer(None, None, ()).unwrap();

    // TODO: is this important?
    // For each of the windows send the frame callbacks to windows telling them to draw.
    //state.space.elements().for_each(|window| {
    //    window.send_frame(
    //        &output,
    //        start_time.elapsed(),
    //        Some(core::time::Duration::ZERO),
    //        |_, _| Some(output.clone()),
    //    )
    //});

    Ok(())
}
