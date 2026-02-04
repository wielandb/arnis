use crate::osm_parser::{
    ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay,
};
use geo::{Contains, LineString, Point, Polygon};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Neighbor distance (meters) used for context generation.
pub const NEIGHBOR_DISTANCE_METERS: f64 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    Node,
    Way,
    Relation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId {
    pub kind: ObjectKind,
    pub id: u64,
}

impl ObjectId {
    pub fn new(kind: ObjectKind, id: u64) -> Self {
        Self { kind, id }
    }

    pub fn node(id: u64) -> Self {
        Self::new(ObjectKind::Node, id)
    }

    pub fn way(id: u64) -> Self {
        Self::new(ObjectKind::Way, id)
    }

    pub fn relation(id: u64) -> Self {
        Self::new(ObjectKind::Relation, id)
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ContextMember {
    pub role: String,
    pub object: Arc<ContextObject>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ContextObject {
    pub id: u64,
    pub kind: ObjectKind,
    pub tags: HashMap<String, String>,
    pub members: Vec<ContextMember>,
    pub children: Vec<Arc<ContextObject>>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct GenerationContext {
    pub parent_ways: Option<Vec<Arc<ContextObject>>>,
    pub connected_ways: Option<Vec<Arc<ContextObject>>>,
    pub children_nodes: Option<Vec<Arc<ContextObject>>>,
    pub parent_relations: Vec<Arc<ContextObject>>,
    pub containing_objects: Option<Vec<Arc<ContextObject>>>,
    pub neighbours: Vec<Arc<ContextObject>>,
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
}

#[derive(Debug)]
pub struct ContextIndex {
    objects: HashMap<ObjectId, Arc<ContextObject>>,
    node_objects: HashMap<u64, Arc<ContextObject>>,
    way_objects: HashMap<u64, Arc<ContextObject>>,
    relation_objects: HashMap<u64, Arc<ContextObject>>,
    node_coords: HashMap<u64, (i32, i32)>,
    object_nodes: HashMap<ObjectId, Vec<u64>>,
    object_bounds: HashMap<ObjectId, Bounds>,
    node_to_ways: HashMap<u64, Vec<u64>>,
    way_to_connected: HashMap<u64, Vec<u64>>,
    way_to_relations: HashMap<u64, Vec<u64>>,
    node_to_objects: HashMap<u64, Vec<ObjectId>>,
    node_grid: HashMap<(i32, i32), Vec<u64>>,
    neighbor_distance_blocks: f64,
}

impl ContextIndex {
    pub fn new(elements: &[ProcessedElement], scale: f64) -> Self {
        let neighbor_distance_blocks = (NEIGHBOR_DISTANCE_METERS * scale).max(0.0);

        let mut nodes_map: HashMap<u64, ProcessedNode> = HashMap::new();
        let mut ways_map: HashMap<u64, ProcessedWay> = HashMap::new();
        let mut relations: Vec<ProcessedRelation> = Vec::new();

        for element in elements {
            match element {
                ProcessedElement::Node(node) => {
                    insert_node(&mut nodes_map, node);
                }
                ProcessedElement::Way(way) => {
                    insert_way(&mut ways_map, way);
                    for node in &way.nodes {
                        insert_node(&mut nodes_map, node);
                    }
                }
                ProcessedElement::Relation(relation) => {
                    relations.push(relation.clone());
                    for member in &relation.members {
                        insert_way(&mut ways_map, &member.way);
                        for node in &member.way.nodes {
                            insert_node(&mut nodes_map, node);
                        }
                    }
                }
            }
        }

        let mut objects: HashMap<ObjectId, Arc<ContextObject>> = HashMap::new();
        let mut node_objects: HashMap<u64, Arc<ContextObject>> = HashMap::new();
        let mut way_objects: HashMap<u64, Arc<ContextObject>> = HashMap::new();
        let mut relation_objects: HashMap<u64, Arc<ContextObject>> = HashMap::new();

        for node in nodes_map.values() {
            let obj = Arc::new(ContextObject {
                id: node.id,
                kind: ObjectKind::Node,
                tags: node.tags.clone(),
                members: Vec::new(),
                children: Vec::new(),
            });
            let obj_id = ObjectId::node(node.id);
            objects.insert(obj_id, Arc::clone(&obj));
            node_objects.insert(node.id, obj);
        }

        for way in ways_map.values() {
            let children: Vec<Arc<ContextObject>> = way
                .nodes
                .iter()
                .filter_map(|node| node_objects.get(&node.id).cloned())
                .collect();

            let obj = Arc::new(ContextObject {
                id: way.id,
                kind: ObjectKind::Way,
                tags: way.tags.clone(),
                members: Vec::new(),
                children,
            });
            let obj_id = ObjectId::way(way.id);
            objects.insert(obj_id, Arc::clone(&obj));
            way_objects.insert(way.id, obj);
        }

        for relation in &relations {
            let mut members = Vec::new();
            for member in &relation.members {
                if let Some(way_obj) = way_objects.get(&member.way.id) {
                    let role = match member.role {
                        ProcessedMemberRole::Outer => "outer",
                        ProcessedMemberRole::Inner => "inner",
                    };
                    members.push(ContextMember {
                        role: role.to_string(),
                        object: Arc::clone(way_obj),
                    });
                }
            }

            let obj = Arc::new(ContextObject {
                id: relation.id,
                kind: ObjectKind::Relation,
                tags: relation.tags.clone(),
                members,
                children: Vec::new(),
            });
            let obj_id = ObjectId::relation(relation.id);
            objects.insert(obj_id, Arc::clone(&obj));
            relation_objects.insert(relation.id, obj);
        }

        let mut node_coords: HashMap<u64, (i32, i32)> = HashMap::new();
        for node in nodes_map.values() {
            node_coords.insert(node.id, (node.x, node.z));
        }

        let mut object_nodes: HashMap<ObjectId, Vec<u64>> = HashMap::new();
        for node in nodes_map.values() {
            object_nodes.insert(ObjectId::node(node.id), vec![node.id]);
        }

        for way in ways_map.values() {
            let node_ids: Vec<u64> = way.nodes.iter().map(|n| n.id).collect();
            object_nodes.insert(ObjectId::way(way.id), node_ids);
        }

        for relation in &relations {
            let mut node_ids: HashSet<u64> = HashSet::new();
            for member in &relation.members {
                for node in &member.way.nodes {
                    node_ids.insert(node.id);
                }
            }
            object_nodes.insert(
                ObjectId::relation(relation.id),
                node_ids.into_iter().collect(),
            );
        }

        let mut object_bounds: HashMap<ObjectId, Bounds> = HashMap::new();
        for (obj_id, node_ids) in &object_nodes {
            if node_ids.is_empty() {
                continue;
            }

            let mut min_x = i32::MAX;
            let mut max_x = i32::MIN;
            let mut min_z = i32::MAX;
            let mut max_z = i32::MIN;
            let mut has_coord = false;

            for node_id in node_ids {
                if let Some((x, z)) = node_coords.get(node_id) {
                    has_coord = true;
                    min_x = min_x.min(*x);
                    max_x = max_x.max(*x);
                    min_z = min_z.min(*z);
                    max_z = max_z.max(*z);
                }
            }

            if has_coord {
                object_bounds.insert(
                    *obj_id,
                    Bounds {
                        min_x,
                        max_x,
                        min_z,
                        max_z,
                    },
                );
            }
        }

        let mut node_to_ways_set: HashMap<u64, HashSet<u64>> = HashMap::new();
        for way in ways_map.values() {
            for node in &way.nodes {
                node_to_ways_set
                    .entry(node.id)
                    .or_default()
                    .insert(way.id);
            }
        }
        let node_to_ways: HashMap<u64, Vec<u64>> = node_to_ways_set
            .into_iter()
            .map(|(node_id, ways)| (node_id, ways.into_iter().collect()))
            .collect();

        let mut way_to_connected: HashMap<u64, Vec<u64>> = HashMap::new();
        for way in ways_map.values() {
            let mut connected: HashSet<u64> = HashSet::new();
            for node in &way.nodes {
                if let Some(ways) = node_to_ways.get(&node.id) {
                    for way_id in ways {
                        if *way_id != way.id {
                            connected.insert(*way_id);
                        }
                    }
                }
            }
            way_to_connected.insert(way.id, connected.into_iter().collect());
        }

        let mut way_to_relations_set: HashMap<u64, HashSet<u64>> = HashMap::new();
        for relation in &relations {
            for member in &relation.members {
                way_to_relations_set
                    .entry(member.way.id)
                    .or_default()
                    .insert(relation.id);
            }
        }
        let way_to_relations: HashMap<u64, Vec<u64>> = way_to_relations_set
            .into_iter()
            .map(|(way_id, relations)| (way_id, relations.into_iter().collect()))
            .collect();

        let mut node_to_objects_set: HashMap<u64, HashSet<ObjectId>> = HashMap::new();
        for (obj_id, node_ids) in &object_nodes {
            for node_id in node_ids {
                node_to_objects_set
                    .entry(*node_id)
                    .or_default()
                    .insert(*obj_id);
            }
        }
        let node_to_objects: HashMap<u64, Vec<ObjectId>> = node_to_objects_set
            .into_iter()
            .map(|(node_id, objects)| (node_id, objects.into_iter().collect()))
            .collect();

        let mut node_grid: HashMap<(i32, i32), Vec<u64>> = HashMap::new();
        let cell_size = neighbor_distance_blocks.max(1.0);
        for (node_id, (x, z)) in &node_coords {
            let cell_x = ((*x as f64) / cell_size).floor() as i32;
            let cell_z = ((*z as f64) / cell_size).floor() as i32;
            node_grid
                .entry((cell_x, cell_z))
                .or_default()
                .push(*node_id);
        }

        Self {
            objects,
            node_objects,
            way_objects,
            relation_objects,
            node_coords,
            object_nodes,
            object_bounds,
            node_to_ways,
            way_to_connected,
            way_to_relations,
            node_to_objects,
            node_grid,
            neighbor_distance_blocks,
        }
    }

    pub fn context_for_element(&self, element: &ProcessedElement) -> GenerationContext {
        match element {
            ProcessedElement::Node(node) => self.context_for_node(node),
            ProcessedElement::Way(way) => self.context_for_way(way),
            ProcessedElement::Relation(relation) => self.context_for_relation(relation),
        }
    }

    pub fn context_for_node(&self, node: &ProcessedNode) -> GenerationContext {
        let object_id = ObjectId::node(node.id);
        let parent_ways = Some(
            self.node_to_ways
                .get(&node.id)
                .map(|ways| self.map_way_ids(ways))
                .unwrap_or_default(),
        );

        let neighbours = self.neighbours_for_nodes(object_id, &[node.id]);

        GenerationContext {
            parent_ways,
            connected_ways: None,
            children_nodes: None,
            parent_relations: Vec::new(),
            containing_objects: None,
            neighbours,
        }
    }

    pub fn context_for_way(&self, way: &ProcessedWay) -> GenerationContext {
        let object_id = ObjectId::way(way.id);
        if let Some(node_ids) = self.object_nodes.get(&object_id) {
            let children_nodes = self
                .way_objects
                .get(&way.id)
                .map(|obj| obj.children.clone())
                .unwrap_or_default();

            let connected_ways = if let Some(ways) = self.way_to_connected.get(&way.id) {
                self.map_way_ids(ways)
            } else if node_ids.is_empty() {
                Vec::new()
            } else {
                let mut connected: HashSet<u64> = HashSet::new();
                for node_id in node_ids {
                    if let Some(ways) = self.node_to_ways.get(node_id) {
                        for way_id in ways {
                            if *way_id != way.id {
                                connected.insert(*way_id);
                            }
                        }
                    }
                }
                self.map_way_ids_from_set(&connected)
            };

            let parent_relations = self
                .way_to_relations
                .get(&way.id)
                .map(|relations| self.map_relation_ids(relations))
                .unwrap_or_default();

            let containing_objects =
                self.containing_objects_for_way_nodes(object_id, node_ids);

            let neighbours = self.neighbours_for_nodes(object_id, node_ids);

            return GenerationContext {
                parent_ways: None,
                connected_ways: Some(connected_ways),
                children_nodes: Some(children_nodes),
                parent_relations,
                containing_objects,
                neighbours,
            };
        }

        let fallback_node_ids: Vec<u64> = way.nodes.iter().map(|n| n.id).collect();
        let children_nodes: Vec<Arc<ContextObject>> = fallback_node_ids
            .iter()
            .filter_map(|id| self.node_objects.get(id).cloned())
            .collect();

        let connected_ways = if fallback_node_ids.is_empty() {
            Vec::new()
        } else {
            let mut connected: HashSet<u64> = HashSet::new();
            for node_id in &fallback_node_ids {
                if let Some(ways) = self.node_to_ways.get(node_id) {
                    for way_id in ways {
                        if *way_id != way.id {
                            connected.insert(*way_id);
                        }
                    }
                }
            }
            self.map_way_ids_from_set(&connected)
        };

        let parent_relations = self
            .way_to_relations
            .get(&way.id)
            .map(|relations| self.map_relation_ids(relations))
            .unwrap_or_default();

        let containing_objects =
            self.containing_objects_for_way_nodes(object_id, &fallback_node_ids);

        let neighbours = self.neighbours_for_nodes(object_id, &fallback_node_ids);

        GenerationContext {
            parent_ways: None,
            connected_ways: Some(connected_ways),
            children_nodes: Some(children_nodes),
            parent_relations,
            containing_objects,
            neighbours,
        }
    }

    pub fn context_for_relation(&self, relation: &ProcessedRelation) -> GenerationContext {
        let object_id = ObjectId::relation(relation.id);
        let neighbours = if let Some(node_ids) = self.object_nodes.get(&object_id) {
            self.neighbours_for_nodes(object_id, node_ids)
        } else {
            let mut ids: HashSet<u64> = HashSet::new();
            for member in &relation.members {
                for node in &member.way.nodes {
                    ids.insert(node.id);
                }
            }
            let fallback_node_ids: Vec<u64> = ids.into_iter().collect();
            self.neighbours_for_nodes(object_id, &fallback_node_ids)
        };

        GenerationContext {
            parent_ways: None,
            connected_ways: None,
            children_nodes: None,
            parent_relations: Vec::new(),
            containing_objects: None,
            neighbours,
        }
    }

    fn map_way_ids(&self, way_ids: &[u64]) -> Vec<Arc<ContextObject>> {
        way_ids
            .iter()
            .filter_map(|id| self.way_objects.get(id).cloned())
            .collect()
    }

    fn map_way_ids_from_set(&self, way_ids: &HashSet<u64>) -> Vec<Arc<ContextObject>> {
        way_ids
            .iter()
            .filter_map(|id| self.way_objects.get(id).cloned())
            .collect()
    }

    fn map_relation_ids(&self, relation_ids: &[u64]) -> Vec<Arc<ContextObject>> {
        relation_ids
            .iter()
            .filter_map(|id| self.relation_objects.get(id).cloned())
            .collect()
    }

    fn is_closed_way_nodes(&self, node_ids: &[u64]) -> bool {
        if node_ids.len() < 3 {
            return false;
        }
        let first = match self.node_coords.get(&node_ids[0]) {
            Some(coord) => coord,
            None => return false,
        };
        let last = match self.node_coords.get(node_ids.last().unwrap()) {
            Some(coord) => coord,
            None => return false,
        };
        first == last
    }

    fn containing_objects_for_way_nodes(
        &self,
        object_id: ObjectId,
        node_ids: &[u64],
    ) -> Option<Vec<Arc<ContextObject>>> {
        if !self.is_closed_way_nodes(node_ids) {
            return None;
        }

        let mut coords: Vec<(f64, f64)> = Vec::with_capacity(node_ids.len());
        for node_id in node_ids {
            if let Some((x, z)) = self.node_coords.get(node_id) {
                coords.push((*x as f64, *z as f64));
            } else {
                return None;
            }
        }

        if coords.len() < 3 {
            return None;
        }

        let (poly_min_x, poly_max_x, poly_min_z, poly_max_z) = coords.iter().fold(
            (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
            |(min_x, max_x, min_z, max_z), (x, z)| {
                (
                    min_x.min(*x),
                    max_x.max(*x),
                    min_z.min(*z),
                    max_z.max(*z),
                )
            },
        );

        let polygon = Polygon::new(LineString::from(coords), vec![]);
        let mut contained: Vec<Arc<ContextObject>> = Vec::new();

        for (other_id, other_nodes) in &self.object_nodes {
            if *other_id == object_id {
                continue;
            }

            if other_nodes.is_empty() {
                continue;
            }

            if let Some(bounds) = self.object_bounds.get(other_id) {
                if (bounds.min_x as f64) < poly_min_x
                    || (bounds.max_x as f64) > poly_max_x
                    || (bounds.min_z as f64) < poly_min_z
                    || (bounds.max_z as f64) > poly_max_z
                {
                    continue;
                }
            }

            let mut all_inside = true;
            for node_id in other_nodes {
                if let Some((x, z)) = self.node_coords.get(node_id) {
                    if !polygon.contains(&Point::new(*x as f64, *z as f64)) {
                        all_inside = false;
                        break;
                    }
                } else {
                    all_inside = false;
                    break;
                }
            }

            if all_inside {
                if let Some(obj) = self.objects.get(other_id) {
                    contained.push(Arc::clone(obj));
                }
            }
        }

        Some(contained)
    }

    fn neighbours_for_nodes(
        &self,
        object_id: ObjectId,
        node_ids: &[u64],
    ) -> Vec<Arc<ContextObject>> {
        if node_ids.is_empty() || self.neighbor_distance_blocks <= 0.0 {
            return Vec::new();
        }

        let mut neighbour_ids: HashSet<ObjectId> = HashSet::new();
        let threshold_sq = self.neighbor_distance_blocks * self.neighbor_distance_blocks;
        let cell_size = self.neighbor_distance_blocks.max(1.0);

        for node_id in node_ids {
            let (x, z) = match self.node_coords.get(node_id) {
                Some(coord) => *coord,
                None => continue,
            };

            let cell_x = ((x as f64) / cell_size).floor() as i32;
            let cell_z = ((z as f64) / cell_size).floor() as i32;

            for dx in -1..=1 {
                for dz in -1..=1 {
                    let cell_key = (cell_x + dx, cell_z + dz);
                    if let Some(cell_nodes) = self.node_grid.get(&cell_key) {
                        for other_node_id in cell_nodes {
                            if let Some((ox, oz)) = self.node_coords.get(other_node_id) {
                                let dx = (x - *ox) as f64;
                                let dz = (z - *oz) as f64;
                                if (dx * dx + dz * dz) <= threshold_sq {
                                    if let Some(objects) = self.node_to_objects.get(other_node_id)
                                    {
                                        for other_id in objects {
                                            if *other_id != object_id {
                                                neighbour_ids.insert(*other_id);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        neighbour_ids
            .into_iter()
            .filter_map(|id| self.objects.get(&id).cloned())
            .collect()
    }
}

fn insert_node(nodes_map: &mut HashMap<u64, ProcessedNode>, node: &ProcessedNode) {
    match nodes_map.get(&node.id) {
        Some(existing) => {
            if existing.tags.is_empty() && !node.tags.is_empty() {
                nodes_map.insert(node.id, node.clone());
            }
        }
        None => {
            nodes_map.insert(node.id, node.clone());
        }
    }
}

fn insert_way(ways_map: &mut HashMap<u64, ProcessedWay>, way: &ProcessedWay) {
    let should_replace = match ways_map.get(&way.id) {
        Some(existing) => way.nodes.len() > existing.nodes.len(),
        None => true,
    };

    if should_replace {
        ways_map.insert(way.id, way.clone());
    }
}
