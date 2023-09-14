use smithay::{
    backend::egl::ffi::egl::types::__eglMustCastToProperFunctionPointerType,
    desktop::{space::SpaceElement, Space, Window},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
    wayland::shell::xdg::ToplevelSurface,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

/// This Struct keeps track of all the tiles
/// in a tree structure
pub struct TilingState {
    // TEST
    pub tile_tree_head: Option<Node>,
    pub tile_info: HashMap<WlSurface, Rc<RefCell<Tile>>>,
}

impl TilingState {
    pub fn init() -> Self {
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
                let tile = Tile {
                    next_split: Split::Vertical,
                    geometry,
                    container: None,
                    side: Side::Unique,
                    window: window.clone(),
                };
                let tile = Rc::new(RefCell::new(tile));
                let node = Node::Tile(Rc::clone(&tile));
                self.tile_tree_head = Some(Node::clone(&node));
                self.tile_info
                    .insert(window.toplevel().wl_surface().clone(), tile);
                node
            }
        })
    }

    /// This method is called on a Tile,
    /// from this tile will be created a Stucture Node containing
    /// two children the current Tile and the new tile (both with updated sizes)
    pub fn split(&mut self, window: Window, new_window: Window) -> Node {
        // Get the Tile that needs to be splited in half
        let tile_to_split = Rc::clone(
            self.tile_info
                .get(window.toplevel().wl_surface())
                .expect("IMP not having a wl_surface in TileInfo"),
        );

        // Create new tile
        let new_tile = Rc::new(RefCell::new(Tile {
            next_split: tile_to_split.borrow().next_split.clone(),
            geometry: Rectangle::default(), // not relevant, to be changed later
            container: None,                // not relevant, to be changed later
            side: Side::Right,
            window: new_window,
        }));

        self.tile_info.insert(
            new_tile.borrow().window.toplevel().wl_surface().clone(),
            Rc::clone(&new_tile),
        );

        // Create structure
        let structure = Rc::new(RefCell::new(Structure {
            geometry: tile_to_split.borrow().geometry,
            container: tile_to_split.borrow().container.clone(),
            side: tile_to_split.borrow().side.clone(),
            split: tile_to_split.borrow().next_split.clone(),
            left: Node::Tile(Rc::clone(&tile_to_split)),
            right: Node::Tile(Rc::clone(&new_tile)),
        }));

        match structure.borrow().container.as_ref() {
            // The upper container must poit to the new struct
            Some(upper_container) => upper_container.borrow_mut().set_side(
                structure.borrow().side,
                &Node::Structure(Rc::clone(&structure)),
            ),
            // update head of the tree
            None => self.tile_tree_head = Some(Node::Structure(Rc::clone(&structure))),
        }

        // Update tiles
        {
            let mut left_tile = tile_to_split.borrow_mut();
            left_tile.container = Some(Rc::clone(&structure));
            left_tile.side = Side::Left;
        }

        new_tile.borrow_mut().container = Some(Rc::clone(&structure));

        // call update size on the structure
        Self::update_geometry_node(Node::Structure(Rc::clone(&structure)), None);
        Node::Structure(structure)
    }

    pub fn set_split(&mut self, wl_surface: &WlSurface, new_split: Split) {
        self.tile_info
            .get_mut(wl_surface)
            .expect("IMP having surface NOT present in tile_info map")
            .borrow_mut()
            .next_split = new_split;
    }

    /// given a wl surface the sibiling node will assume the geometry of the container
    /// the container will be eliminated and the upper container will point to the remaining Tile
    pub fn destroy(&mut self, wl_surface: &WlSurface) -> Result<Option<Node>, &'static str> {
        // get the tile to be destroyed
        let tile_to_destroy = self
            .tile_info
            .remove(wl_surface)
            .expect("IMP having surface NOT present in tile_info map");

        // Get the sibiling that should cover the all the destroyed space
        let container = match tile_to_destroy.borrow().container.as_ref() {
            // The container is a normal Structure
            Some(c) => Rc::clone(c),
            // If the container is not present then
            // the tile is unique, just needed to  remove the head of the Tree
            None => {
                println!("REMOVE LAST TILE");
                self.tile_tree_head = None;
                return Ok(None);
            }
        };
        let mut sibiling = Node::get_sibiling(&container.borrow(), tile_to_destroy.borrow().side);

        // We have two cases now:
        // + The sibilign is a Tile
        // + The sibiling is a Structure

        let upper_container = container.borrow().container.clone();
        // Copy the geometry from the container
        sibiling.set_geometry(container.borrow().geometry);
        // Update the container of the tile
        sibiling.set_container(upper_container.clone());
        sibiling.set_side(container.borrow().side);

        match upper_container.as_ref() {
            // the upper container will be the new container of the remaining tile
            Some(upper_container) => {
                // Make the upper container pointing to the remaining tile
                upper_container
                    .borrow_mut()
                    .set_side(container.borrow().side, &sibiling);
            }
            // If there's no upper container then the tile
            // will become the head of the tile tree
            None => {
                self.tile_tree_head = Some(Node::clone(&sibiling));
            }
        };

        if let Node::Structure(_) = sibiling {
            Self::update_geometry_node(Node::clone(&sibiling), None);
        }
        Ok(Some(Node::clone(&sibiling)))
    }

    /// This function will accept a Node and update all the subtree geometry with the new
    /// geometry specified, nothing will be changed except the field geometry
    ///
    /// if None then every node in the subtree will be reevaluated with the current geometry
    /// in the passed node
    pub fn update_geometry_node(node: Node, new_geometry: Option<Rectangle<i32, Logical>>) {
        match node {
            Node::Structure(structure) => {
                // if new geometry is specified then they are applied to the
                // structure before upfate all the subtree geometries
                if let Some(new_geom) = new_geometry {
                    structure.borrow_mut().geometry = new_geom;
                }

                let structure = structure.borrow();
                // TODO: How can I avoid this two clones?
                let mut left_node = Node::clone(&structure.left);
                let mut right_node = Node::clone(&structure.right);

                match structure.split {
                    Split::Horizontal => {
                        let new_width = (structure.geometry.size.w as f32 / 2.0).floor() as i32;
                        let mut left_geom = structure.geometry;
                        left_geom.size.w = new_width;
                        left_node.set_geometry(left_geom);

                        let right_geom = Rectangle::from_loc_and_size(
                            (left_geom.loc.x + new_width, left_geom.loc.y),
                            left_geom.size,
                        );
                        right_node.set_geometry(right_geom);
                    }
                    Split::Vertical => {
                        let new_height = (structure.geometry.size.h as f32 / 2.0).floor() as i32;
                        let mut left_geom = structure.geometry;
                        left_geom.size.h = new_height;
                        left_node.set_geometry(left_geom);

                        let right_geom = Rectangle::from_loc_and_size(
                            (left_geom.loc.x, left_geom.loc.y + new_height),
                            left_geom.size,
                        );
                        right_node.set_geometry(right_geom);
                    }
                }

                // recursive if left or right sons are Strucutre
                let recursive_if_structure = |node: Node| match node {
                    Node::Structure(_) => Self::update_geometry_node(node, None),
                    _ => (),
                };
                recursive_if_structure(left_node);
                recursive_if_structure(right_node);
            }
            // That's NOT so stupid, when you have only two window
            // and you destroy on of the two then the last node
            // remained is a Tile and it should update the sizes here ?
            Node::Tile(_) => panic!("you stupid?"),
        }
    }

    /// This function should update the space
    /// of all the subtree under the node
    pub fn update_space(&self, node: Node, space: &mut Space<Window>) {
        match node {
            Node::Structure(structure) => {
                self.update_space(Node::clone(&structure.borrow().left), space);
                self.update_space(Node::clone(&structure.borrow().right), space);
            }
            Node::Tile(tile) => {
                println!("TILE: {tile:?}");
                tile.borrow()
                    .window
                    .toplevel()
                    .with_pending_state(|top_level_state| {
                        top_level_state.bounds = Some(tile.borrow().geometry.size);
                        top_level_state.size = Some(tile.borrow().geometry.size);
                        // here could be setted also the decoration mode
                    });
                // TODO: find a way to avoid sending figure if
                // the window is just created
                tile.borrow().window.toplevel().send_configure();
                // TODO: ACTIVATE???
                space.map_element(
                    tile.borrow().window.clone(),
                    tile.borrow().geometry.loc,
                    false,
                );
            }
        }
    }
}

// The derive clone should use the clone of Rc,
// then I can direcly use Node::clone istead of pattern matching
// and the Rc::clone the body (maybe)
#[derive(Clone, Debug)]
pub enum Node {
    Structure(Rc<RefCell<Structure>>),
    Tile(Rc<RefCell<Tile>>),
}

impl Node {
    fn set_geometry(&mut self, new_geometry: Rectangle<i32, Logical>) {
        match self {
            Node::Structure(s) => s.borrow_mut().geometry = new_geometry,
            Node::Tile(t) => t.borrow_mut().geometry = new_geometry,
        }
    }

    fn set_container(&mut self, new_container: Option<Rc<RefCell<Structure>>>) {
        match self {
            Node::Structure(s) => s.borrow_mut().container = new_container,
            Node::Tile(t) => t.borrow_mut().container = new_container,
        }
    }

    fn set_side(&mut self, new_side: Side) {
        match self {
            Node::Structure(s) => s.borrow_mut().side = new_side,
            Node::Tile(t) => t.borrow_mut().side = new_side,
        }
    }

    fn get_sibiling(container: &Structure, side: Side) -> Node {
        match side {
            Side::Left => container.right.clone(),
            Side::Right => container.left.clone(),
            Side::Unique => panic!("WAJKHSAKJDHAd"),
        }
    }
}

#[derive(Clone)]
pub enum Split {
    Vertical,
    Horizontal,
}

#[derive(Clone)]
pub struct Structure {
    geometry: Rectangle<i32, Logical>,
    container: Option<Rc<RefCell<Structure>>>,
    side: Side,
    split: Split,
    left: Node,
    right: Node,
}

impl std::fmt::Debug for Structure {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "Strcuture: \n \
             geometry: \n{:?}\n \
             left: \n{:?}\n \
             right: \n{:?}",
            self.geometry, self.left, self.right
        )
    }
}
impl Structure {
    fn set_side(&mut self, side: Side, node: &Node) {
        match side {
            Side::Right => {
                self.right = Node::clone(node);
            }
            Side::Left => {
                self.left = Node::clone(node);
            }
            Side::Unique => {
                panic!("IMP Structure has only left and right sons")
            }
        };
    }
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
    Unique,
}

#[derive(Clone)]
pub struct Tile {
    next_split: Split,
    geometry: Rectangle<i32, Logical>,
    // The container of a Tile can ONLY be a structure
    container: Option<Rc<RefCell<Structure>>>,
    side: Side,
    window: Window,
}

impl std::fmt::Debug for Tile {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "Tile: geometry: {:?}, container_is_none: {}",
            self.geometry,
            self.container.is_none()
        )
    }
}
