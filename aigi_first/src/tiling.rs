use smithay::{
    desktop::{space::SpaceElement, Space, Window},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
    wayland::shell::xdg::ToplevelSurface,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

pub struct TilingState {
    tile_tree_head: Option<Node>,
    // TODO: maybe here should be: HashMap<WlSurface, Node>
    tile_info: HashMap<WlSurface, Rc<RefCell<Tile>>>,
}

impl TilingState {
    pub fn new() -> Self {
        Self {
            tile_tree_head: None,
            tile_info: HashMap::new(),
        }
    }

    pub fn insert_head(
        &mut self,
        window: Window,
        geometry: Rectangle<i32, Logical>,
    ) -> Result<Node, &'static str> {
        Ok(match self.tile_tree_head {
            Some(_) => return Err("WOOOOOW head already exists"),
            None => {
                let tile = Tile::new(
                    Split::Horizontal,
                    geometry,
                    None,
                    Sibiling::Unique,
                    window.clone(),
                );
                let new_node = Node::Leaf(Rc::new(RefCell::new(tile)));
                self.tile_tree_head = Some(Node::clone(&new_node));
                self.tile_info.insert(
                    window.toplevel().wl_surface().clone(),
                    self.tile_tree_head.as_ref().unwrap().get_rc_tile().unwrap(),
                );
                new_node
            }
        })
    }

    /// This method is called on a Tile,
    /// from this tile will be created a Stucture Node containing
    /// two children the current Tile and the new tile (both with updated sizes)
    pub fn split(&mut self, to_split_window: Window, new_window: Window) -> Node {
        // Get the Tile that needs to be splited in half
        let container_geometry: Rectangle<i32, Logical>;
        let upper_container: Option<Node>;
        let prev_sibiling: Sibiling;
        let tile_to_split = self
            .tile_info
            .get(to_split_window.toplevel().wl_surface())
            .expect("IMP not having a wl_surface in TileInfo");
        println!("Tile to split: {tile_to_split:?}");
        let new_tile: Rc<RefCell<Tile>>;

        // Create this scope to being able to
        // mofdify internally tile_to_split
        // and later copy it to be inserted correctly in the
        // new structure node
        {
            let mut tile_to_split = tile_to_split.borrow_mut();

            // Clone the info that will be stored in the new container
            container_geometry = tile_to_split.geometry.clone();
            // TODO: Could be nice to avoid cloning the stuff here
            // but moving the memory directly without cloning
            upper_container = tile_to_split
                .container
                .as_ref()
                .and_then(|c| Some(Node::clone(c)))
                .clone();
            prev_sibiling = tile_to_split.sibiling.clone();

            match tile_to_split.split {
                Split::Horizontal => {
                    // update tile_to_split
                    let new_width = (container_geometry.size.w as f32 / 2.0).floor() as i32;
                    tile_to_split.geometry.size.w = new_width;
                    tile_to_split.sibiling = Sibiling::Left;
                    tile_to_split.container = None;

                    // create new tile
                    let new_tile_geometry = Rectangle::from_loc_and_size(
                        (
                            tile_to_split.geometry.loc.x + new_width,
                            tile_to_split.geometry.loc.y,
                        ),
                        tile_to_split.geometry.size,
                    );
                    new_tile = Rc::new(RefCell::new(Tile::new(
                        tile_to_split.split.clone(),
                        new_tile_geometry,
                        None,
                        Sibiling::Right,
                        new_window,
                    )));
                    println!("Tiles should be created \n left tile: {tile_to_split:?} \n right tile: {new_tile:?}");
                }
                Split::Vertical => todo!(),
            }
        }

        // Create Structure Node
        let structure_node = Node::Internal(Rc::new(RefCell::new(Structure::new(
            container_geometry,
            upper_container.clone(),
            Node::Leaf(Rc::clone(&tile_to_split)), // left
            Node::Leaf(Rc::clone(&new_tile)),      // right
        ))));

        println!("Structure node: {structure_node:?}");

        // update tile and new tile
        tile_to_split.borrow_mut().container = Some(Node::clone(&structure_node));
        new_tile.borrow_mut().container = Some(Node::clone(&structure_node));

        // update the upper container to poit to the new structure node
        // TODO: check the correctness
        if let Some(Node::Internal(upper_structure)) = upper_container {
            match prev_sibiling {
                Sibiling::Left => {
                    upper_structure.borrow_mut().left = Node::clone(&structure_node);
                }
                Sibiling::Right => {
                    upper_structure.borrow_mut().right = Node::clone(&structure_node);
                }
                _ => panic!("Should be impossible"),
            }
        }
        Node::clone(&structure_node)
    }

    /// This method is called on a Tile,
    /// from this tile will be created a Stucture Node containing
    /// two children the current Tile and the new tile (both with updated sizes)
    pub fn destroy(&mut self, new_surface: ToplevelSurface) {}

    /// This function should update the space
    /// of all the subtree under the node
    pub fn update_space(&self, node: Node, space: &mut Space<Window>) {
        match node {
            Node::Internal(structure) => {
                let structure = structure.borrow();
                // TODO: REMOVE THIS CLONE
                println!("INTERNAL");
                self.update_space(structure.left.clone(), space);
                panic!("LEFT DONE");
                println!("LEFT DONE");
                self.update_space(structure.right.clone(), space);
                println!("RIGHT DONE");
            }
            Node::Leaf(tile) => {
                println!("ENTER TILE");
                let tile = dbg!(tile.borrow());
                tile.window
                    .toplevel()
                    .with_pending_state(|top_level_state| {
                        top_level_state.bounds = Some(tile.geometry.size);
                        top_level_state.size = Some(tile.geometry.size);
                        // here could be setted also the decoration mode
                    });
                // TODO: find a way to avoid sending figure if
                // the window is just created
                tile.window.toplevel().send_configure();
                // TODO: ACTIVATE???
                space.map_element(tile.window.clone(), tile.geometry.loc, true);
            }
        }
    }
}

// The derive clone should use the clone of Rc,
// then I can direcly use Node::clone istead of pattern matching
// and the Rc::clone the body (maybe)
#[derive(Clone, Debug)]
pub enum Node {
    Internal(Rc<RefCell<Structure>>),
    Leaf(Rc<RefCell<Tile>>),
}

impl Node {
    fn get_rc_tile(&self) -> Option<Rc<RefCell<Tile>>> {
        if let Node::Leaf(tile) = self {
            return Some(Rc::clone(tile));
        }
        return None;
    }
}

#[derive(Clone, Debug)]
pub enum Split {
    Vertical,
    Horizontal,
}

#[derive(Clone, Debug)]
pub struct Structure {
    geometry: Rectangle<i32, Logical>,
    container: Option<Node>,
    left: Node,
    right: Node,
}

impl Structure {
    fn new(
        geometry: Rectangle<i32, Logical>,
        container: Option<Node>,
        left: Node,
        right: Node,
    ) -> Self {
        Structure {
            geometry,
            container,
            left,
            right,
        }
    }
}

#[derive(Clone, Debug)]
enum Sibiling {
    Left,
    Right,
    Unique,
}

#[derive(Clone, Debug)]
pub struct Tile {
    split: Split,
    geometry: Rectangle<i32, Logical>,
    // TODO: (?) this could also be direclty ad Rc<RefCell<Structure>>
    // becase containers of tiles can only be Structures
    container: Option<Node>,
    sibiling: Sibiling,
    window: Window,
}

impl Tile {
    fn new(
        split: Split,
        geometry: Rectangle<i32, Logical>,
        container: Option<Node>,
        sibiling: Sibiling,
        window: Window,
    ) -> Self {
        Tile {
            split,
            geometry,
            container,
            sibiling,
            window,
        }
    }
}
