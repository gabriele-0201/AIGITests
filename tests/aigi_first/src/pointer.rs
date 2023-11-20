use smithay::{
    backend::renderer::{
        element::{
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            texture::{TextureBuffer, TextureRenderElement},
            AsRenderElements,
        },
        ImportAll, ImportMem, Renderer, Texture,
    },
    input::pointer::CursorImageStatus,
    render_elements,
    utils::{Clock, Monotonic, Physical, Point, Scale, Transform},
};
use std::{collections::BTreeMap, env::var, fs::File, io::Read, ops::Bound, time::Duration};
use xcursor::{parser::parse_xcursor, CursorTheme};

pub struct PointerElement<T: Texture> {
    texture: Option<TextureBuffer<T>>,
    status: CursorImageStatus,
}

impl<T: Texture> Default for PointerElement<T> {
    fn default() -> Self {
        Self {
            texture: Default::default(),
            status: CursorImageStatus::Default,
        }
    }
}

impl<T: Texture> PointerElement<T> {
    pub fn set_status(&mut self, status: CursorImageStatus) {
        self.status = status;
    }

    pub fn set_texture(&mut self, texture: TextureBuffer<T>) {
        self.texture = Some(texture);
    }
}

// This macro combines the two possible elements into one, a WaylandSurfaceRenderElement which
// is provided by the client, or the TextureRenderElement which is the default cursor.
render_elements! {
    pub PointerRenderElement<R> where
        R: ImportAll;
    Surface=WaylandSurfaceRenderElement<R>,
    Texture=TextureRenderElement<<R as Renderer>::TextureId>,
}

// Implement the AsRenderElements which determines which of the elements should be rendered, the
// default cursor or the cursor provided by the client.
impl<T: Texture + Clone + 'static, R> AsRenderElements<R> for PointerElement<T>
where
    R: Renderer<TextureId = T> + ImportAll,
{
    type RenderElement = PointerRenderElement<R>;
    fn render_elements<E>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<E>
    where
        E: From<PointerRenderElement<R>>,
    {
        match &self.status {
            CursorImageStatus::Hidden => vec![],
            CursorImageStatus::Default => {
                if let Some(texture) = self.texture.as_ref() {
                    vec![PointerRenderElement::<R>::from(
                        TextureRenderElement::from_texture_buffer(
                            location.to_f64(),
                            texture,
                            None,
                            None,
                            None,
                        ),
                    )
                    .into()]
                } else {
                    vec![]
                }
            }
            CursorImageStatus::Surface(surface) => {
                let elements: Vec<PointerRenderElement<R>> =
                    smithay::backend::renderer::element::surface::render_elements_from_surface_tree(
                        renderer, surface, location, scale, alpha,
                    );
                elements.into_iter().map(E::from).collect()
            }
        }
    }
}
