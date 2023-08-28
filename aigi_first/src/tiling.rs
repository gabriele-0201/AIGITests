use smithay::{
    desktop::{space::SpaceElement, Space, Window},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle},
    wayland::shell::xdg::ToplevelSurface,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

/// This Struct keeps track of all the tiles
/// in a tree structure
pub struct TilingState {
    tile_tree_head: Option<Node>,
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

        // The upper container must poit to the new struct
        if let Some(upper_container) = structure.borrow().container.as_ref() {
            match structure.borrow().side {
                Side::Left => {
                    upper_container.borrow_mut().left = Node::Structure(Rc::clone(&structure))
                }
                Side::Right => {
                    upper_container.borrow_mut().right = Node::Structure(Rc::clone(&structure))
                }
                Side::Unique => panic!("Impossible to have a container with Side::Unique"),
            }
        } else {
            // update head of the tree
            self.tile_tree_head = Some(Node::Structure(Rc::clone(&structure)));
        };

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

    pub fn change_split(&mut self, wl_surface: &WlSurface, new_split: Split) {
        self.tile_info
            .get_mut(wl_surface)
            .expect("IMP having surface NOT present in tile_info map")
            .borrow_mut()
            .next_split = new_split;
    }

    /// given a wl surface the sibiling node will assume the geometry of the container
    /// the container will be eliminated and the upper container will point to the remaining Tile
    pub fn destroy(&mut self, wl_surface: &WlSurface) -> Result<Option<Node>, &'static str> {
        // TODO
        // get the tile to be destroyed
        let tile_to_destroy = self
            .tile_info
            .get(wl_surface)
            .expect("IMP having surface NOT present in tile_info map")
            .clone();

        // Get the sibiling that should cover the all the destroyed space
        //
        // We have two cases now:
        // + The sibilign is a Tile
        // + The sibiling is a Structure

        let tile_to_destroy = tile_to_destroy.borrow();
        let (container, sibiling) = match tile_to_destroy.container.as_ref() {
            // The container is a normal Structure
            Some(c) => (c, Node::get_sibiling(&c.borrow(), tile_to_destroy.side)),
            // If the container is not present then
            // the tile is unique, just needed to remove the tile
            // from the Map and remove the head of the Tree
            None => {
                return {
                    self.tile_info.remove(wl_surface);
                    self.tile_tree_head = None;
                    Ok(None)
                }
            }
        };

        // Two cases, the sibiling node is a Tile or a Structure
        match sibiling {
            Node::Tile(ref tile) => {
                let mut tile = tile.borrow_mut();
                tile.geometry = container.borrow().geometry;
                tile.container = container.borrow().container.clone();

                match tile.container.as_ref() {
                    // the upper container will be the new container of the remaining tile
                    Some(upper_container) => {
                        // place the tile where the container was in the upper container
                        match container.borrow().side {
                            Side::Right => {
                                upper_container.borrow_mut().right = Node::clone(&sibiling)
                            }
                            Side::Left => {
                                upper_container.borrow_mut().left = Node::clone(&sibiling)
                            }
                            Side::Unique => {
                                panic!("WHAT? the upper container can't be Sibiling::unique (or am I wrong?)")
                            }
                        }
                    }
                    // If the sibiling of a container is Unique then it must be the first
                    // node of the tree
                    // Do nothing here
                    None => (),
                };

                Ok(Some(Node::clone(&sibiling)))
            }
            Node::Structure(structure) => {
                todo!()
                /*
                let left_node = Node::clone(&structure.borrow().left);
                let right_node = Node::clone(&structure.borrow().right);

                let upper_container = match container.borrow().container.as_ref() {
                    Some(c) => Node::clone(c),
                    // if the upper container is none then the first container is the head
                    // and should be used as main container being the meximum size
                    None => {
                        println!("out structure geom: {:?}", container.borrow().geometry);
                        println!("inner structure geom: {:?}", structure.borrow().geometry);
                        structure.borrow_mut().geometry = container.borrow().geometry;
                        let new_head = Node::Internal(Rc::clone(&structure));
                        self.tile_tree_head = Some(Node::clone(&new_head));
                        println!("new structure geom: {:?}", structure.borrow().geometry);
                        new_head
                    }
                };

                // change the container of left and right node
                let change_container = |node: Node, new_container: Node| match node {
                    Node::Leaf(leaf) => leaf.borrow_mut().container = Some(new_container),
                    Node::Internal(structure) => {
                        structure.borrow_mut().container = Some(new_container)
                    }
                };
                change_container(Node::clone(&left_node), Node::clone(&upper_container));
                change_container(Node::clone(&right_node), Node::clone(&upper_container));

                // this should always be true
                if let Node::Internal(c) = Node::clone(&upper_container) {
                    c.borrow_mut().left = left_node;
                    c.borrow_mut().right = right_node;
                }

                Self::update_geometry_node(&upper_container, None);
                Ok(Some(Node::clone(&upper_container)))
                */
            }
        }
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
                let left_node = Node::clone(&structure.left);
                let right_node = Node::clone(&structure.right);

                match structure.split {
                    Split::Horizontal => {
                        let new_width = (structure.geometry.size.w as f32 / 2.0).floor() as i32;
                        let mut left_geom = dbg!(structure.geometry);
                        left_geom.size.w = new_width;
                        left_node.change_geometry(dbg!(left_geom));

                        let right_geom = Rectangle::from_loc_and_size(
                            (left_geom.loc.x + new_width, left_geom.loc.y),
                            left_geom.size,
                        );
                        right_node.change_geometry(dbg!(right_geom));
                    }
                    Split::Vertical => {
                        let new_height = (structure.geometry.size.h as f32 / 2.0).floor() as i32;
                        let mut left_geom = structure.geometry;
                        left_geom.size.h = new_height;
                        left_node.change_geometry(left_geom);

                        let right_geom = Rectangle::from_loc_and_size(
                            (left_geom.loc.x, left_geom.loc.y + new_height),
                            left_geom.size,
                        );
                        right_node.change_geometry(right_geom);
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
                println!("Structure");
                self.update_space(Node::clone(&structure.borrow().left), space);
                self.update_space(Node::clone(&structure.borrow().right), space);
            }
            Node::Tile(tile) => {
                println!("Tile");
                tile.borrow()
                    .window
                    .toplevel()
                    .with_pending_state(|top_level_state| {
                        top_level_state.bounds = Some(dbg!(tile.borrow().geometry).size);
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
#[derive(Clone)]
pub enum Node {
    Structure(Rc<RefCell<Structure>>),
    Tile(Rc<RefCell<Tile>>),
}

impl Node {
    fn change_geometry(&self, new_geometry: Rectangle<i32, Logical>) {
        match self {
            Node::Structure(s) => s.borrow_mut().geometry = new_geometry,
            Node::Tile(t) => t.borrow_mut().geometry = new_geometry,
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
struct Structure {
    geometry: Rectangle<i32, Logical>,
    container: Option<Rc<RefCell<Structure>>>,
    side: Side,
    split: Split,
    left: Node,
    right: Node,
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
    Unique,
}

#[derive(Clone)]
struct Tile {
    next_split: Split,
    geometry: Rectangle<i32, Logical>,
    // The container of a Tile can ONLY be a structure
    container: Option<Rc<RefCell<Structure>>>,
    side: Side,
    window: Window,
}
