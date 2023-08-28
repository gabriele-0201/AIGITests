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
                    Split::Vertical,
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
            .expect("IMP not having a wl_surface in TileInfo")
            .clone();
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
                }
                Split::Vertical => {
                    // update tile_to_split
                    let new_height = (container_geometry.size.h as f32 / 2.0).floor() as i32;
                    tile_to_split.geometry.size.h = new_height;
                    tile_to_split.sibiling = Sibiling::Left;
                    tile_to_split.container = None;

                    // create new tile
                    let new_tile_geometry = Rectangle::from_loc_and_size(
                        (
                            tile_to_split.geometry.loc.x,
                            tile_to_split.geometry.loc.y + new_height,
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
                }
            }
        }

        // Insert the new Window and Tile in the Map
        self.tile_info.insert(
            new_tile.borrow().window.toplevel().wl_surface().clone(),
            Rc::clone(&new_tile),
        );

        // Create Structure Node
        let structure_node = Node::Internal(Rc::new(RefCell::new(Structure::new(
            container_geometry,
            upper_container.clone(),
            Node::Leaf(Rc::clone(&tile_to_split)), // left
            Node::Leaf(Rc::clone(&new_tile)),      // right
            prev_sibiling.clone(), // sibiling is inherit from the sibilign side of the tile the split started from
            tile_to_split.borrow().split.clone(),
        ))));

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

    pub fn change_split(&mut self, wl_surface: &WlSurface, new_split: Split) {
        self.tile_info
            .get_mut(wl_surface)
            .expect("IMP having surface NOT present in tile_info map")
            .borrow_mut()
            .split = new_split;
    }

    /// given a wl surface the sibiling node will assume the geometry of the container
    /// the container will be eliminated and the upper container will point to the remaining Tile
    pub fn destroy(&mut self, wl_surface: &WlSurface) -> Result<Option<Node>, &'static str> {
        // TODO

        // get the tile to be destroyed
        let tile_to_destroy = self
            .tile_info
            .get_mut(wl_surface)
            .expect("IMP having surface NOT present in tile_info map")
            .borrow();

        // Get the sibiling that should cover the all the destroyed space
        //
        // We have two cases now:
        // + The sibilign is a Tile
        // + The sibiling is a Structure

        let container = match tile_to_destroy.container.as_ref() {
            // The container is a normal Structure
            Some(Node::Internal(c)) => c,
            // If the container is not present then
            // the tile is unique
            None => return Ok(None),
            // If the container is a tile
            // then there is something wrong
            Some(_) => panic!("WHAT!? the container CAN'T be a tile"),
        };

        let sibiling_node = match tile_to_destroy.sibiling {
            Sibiling::Left => Node::clone(&container.borrow().right),
            Sibiling::Right => Node::clone(&container.borrow().left),
            Sibiling::Unique => {
                panic!("Unique tile should be already handled in the previous expression")
            }
        };

        // Two cases, the sibiling node is a Tile or a Structure
        match Node::clone(&sibiling_node) {
            Node::Leaf(tile) => {
                let mut tile = tile.borrow_mut();
                tile.geometry = container.borrow().geometry;
                tile.container = container.borrow().container.clone();

                match tile.container.as_ref() {
                    // the upper container will be the new container of the remaining tile
                    Some(Node::Internal(upper_container)) => {
                        // place the tile where the container was in the upper container
                        match container.borrow().sibiling {
                            Sibiling::Right => {
                                upper_container.borrow_mut().right = Node::clone(&sibiling_node)
                            }
                            Sibiling::Left => {
                                upper_container.borrow_mut().left = Node::clone(&sibiling_node)
                            }
                            Sibiling::Unique => {
                                panic!("WHAT? the upper container can't be Sibiling::unique (or am I wrong?)")
                            }
                        }
                    }
                    Some(Node::Leaf(_)) => {
                        panic!("someting broken")
                    }
                    // If the sibiling of a container is Unique then it must be the first
                    // node of the tree
                    // Do nothing here
                    None => (),
                };

                Ok(Some(sibiling_node))
            }
            Node::Internal(structure) => {
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
            }
        }
    }

    /// This function will accept a Node and update all the subtree geometry with the new
    /// geometry specified, nothing will be changed except the field geometry
    ///
    /// if None then every node in the subtree will be reevaluated with the current geometry
    /// in the passed node
    pub fn update_geometry_node(node: &Node, new_geometry: Option<Rectangle<i32, Logical>>) {
        match node {
            Node::Internal(structure) => {
                let structure = structure.borrow();

                let left_node = Node::clone(&structure.left);
                let right_node = Node::clone(&structure.right);

                let get_geometry = |node: &Node| match node {
                    Node::Internal(s) => s.borrow().geometry,
                    Node::Leaf(t) => t.borrow().geometry,
                };

                let change_geometry =
                    |node: &Node, new_geom: Rectangle<i32, Logical>| match Node::clone(node) {
                        Node::Internal(s) => {
                            s.borrow_mut().geometry = new_geom;
                        }
                        Node::Leaf(t) => {
                            t.borrow_mut().geometry = new_geom;
                        }
                    };

                match structure.split {
                    Split::Horizontal => {
                        // update tile_to_split
                        dbg!(structure.geometry);
                        let new_width = (structure.geometry.size.w as f32 / 2.0).floor() as i32;
                        let mut left_geom = structure.geometry;
                        dbg!(left_geom);
                        left_geom.size.w = new_width;
                        dbg!(left_geom);
                        change_geometry(&left_node, left_geom);

                        // create new tile
                        let right_geom = Rectangle::from_loc_and_size(
                            (left_geom.loc.x + new_width, left_geom.loc.y),
                            left_geom.size,
                        );
                        dbg!(right_geom);
                        change_geometry(&right_node, right_geom);
                    }
                    Split::Vertical => {
                        let new_height = (structure.geometry.size.h as f32 / 2.0).floor() as i32;
                        let mut left_geom = structure.geometry;
                        left_geom.size.h = new_height;
                        change_geometry(&left_node, left_geom);

                        // create new tile
                        let right_geom = Rectangle::from_loc_and_size(
                            (left_geom.loc.x, left_geom.loc.y + new_height),
                            left_geom.size,
                        );
                        change_geometry(&right_node, right_geom);
                    }
                }

                // recursive if left or right sons are Strucutre
                let rec = |node: &Node| match node {
                    Node::Internal(_) => Self::update_geometry_node(node, None),
                    _ => (),
                };
                rec(&left_node);
                rec(&right_node);
            }
            Node::Leaf(_) => panic!("you stupid?"),
        }
    }

    /// This function should update the space
    /// of all the subtree under the node
    pub fn update_space(&self, node: Node, space: &mut Space<Window>) {
        match node {
            Node::Internal(structure) => {
                let structure = structure.borrow();
                println!("ENTER STRUCTURE");
                // TODO: REMOVE THIS CLONE
                self.update_space(structure.left.clone(), space);
                self.update_space(structure.right.clone(), space);
            }
            Node::Leaf(tile) => {
                println!("ENTER TILE");
                let tile = tile.borrow();
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
                space.map_element(tile.window.clone(), tile.geometry.loc, false);
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
    sibiling: Sibiling,
    split: Split,
}

impl Structure {
    fn new(
        geometry: Rectangle<i32, Logical>,
        container: Option<Node>,
        left: Node,
        right: Node,
        sibiling: Sibiling,
        split: Split,
    ) -> Self {
        Structure {
            geometry,
            container,
            left,
            right,
            sibiling,
            split,
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
