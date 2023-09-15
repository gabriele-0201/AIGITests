use std::time::Duration;

use smithay::{
    backend::renderer::{
        damage::OutputDamageTracker,
        element::AsRenderElements,
        gles::{GlesRenderer, GlesTexture},
        multigpu::{gbm::GbmGlesBackend, MultiRenderer, MultiTexture},
        Bind, ImportAll, ImportMem,
    },
    desktop::{space::SpaceRenderElements, Window},
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::Scale,
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
    let gbm_surface = &mut state.backend_data.device_data.gbm_surface;
    gbm_surface.frame_submitted();

    // The Output needs to be extracted by the space,
    // there is only one so we will extract the first one
    let output = state
        .space
        .outputs()
        .next()
        .expect("Impossible not having an output mapped in the Space");

    // Here should be created a time to let the clients render their frames
    let timer = match output.current_mode() {
        Some(mode) => Timer::from_duration(Duration::from_millis(
            ((1_000_000f32 / mode.refresh as f32) * 0.6f32) as u64,
        )),
        None => return Err("Mode not setted in the output".into()),
    };

    state
        .handle
        .insert_source(timer, move |_, _, loop_data| {
            render_new_frame(&mut loop_data.state);
            TimeoutAction::Drop
        })
        .expect("failed to schedule frame timer");

    Ok(())
}

pub fn render_new_frame<'a, 'b>(state: &mut AIGIState) -> Result<(), Box<dyn std::error::Error>> {
    //AGAIN!?!?!?!?!
    let gbm_surface = &mut state.backend_data.device_data.gbm_surface;
    gbm_surface.frame_submitted();

    // The Output needs to be extracted by the space,
    // there is only one so we will extract the first one
    let output = state
        .space
        .outputs()
        .next()
        .expect("Impossible not having an output mapped in the Space");

    let mut renderer = state
        .backend_data
        .gpu_manager
        .single_renderer(&state.backend_data.device_data.render_node)
        .unwrap();

    // NOW LET'S PREPARE ALL THE ELEMENTS
    // only two sets for now, the cursor image and the one present in the Space

    // An element that renders the pointer when rendering the output to display.
    let mut pointer_element = PointerElement::<MultiTexture>::default();

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
    let custom_elements = pointer_element
        .render_elements::<PointerRenderElement<UdevRenderer<'a, 'b>>>(
            &mut renderer,
            cursor_pos_scaled,
            scale,
            1.0,
        );

    //let space_elements = smithay::desktop::space::space_render_elements::<_, Window, _>(
    //    &mut renderer,
    //    [&state.space],
    //    output,
    //    1.0,
    //)
    //.expect("output without mode?");

    //let mut elements: Vec<OutputRenderElements<_, _>> = Vec::new();
    //elements.extend(custom_elements.into_iter().map(OutputRenderElements::from));
    //elements.extend(space_elements.into_iter().map(OutputRenderElements::from));

    // Now that we have all the elements we can get the DMABuf where
    // we will render all those elements

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
    )?;

    gbm_surface.queue_buffer(None, Some(vec![]), ());

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
