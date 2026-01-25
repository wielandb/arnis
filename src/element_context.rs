use crate::coordinate_system::cartesian::XZBBox;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug)]
struct Bounds {
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
}

impl Bounds {
    fn from_nodes(nodes: &[ProcessedNode]) -> Option<Self> {
        let first = nodes.first()?;
        let mut min_x = first.x;
        let mut max_x = first.x;
        let mut min_z = first.z;
        let mut max_z = first.z;

        for node in nodes.iter().skip(1) {
            if node.x < min_x {
                min_x = node.x;
            }
            if node.x > max_x {
                max_x = node.x;
            }
            if node.z < min_z {
                min_z = node.z;
            }
            if node.z > max_z {
                max_z = node.z;
            }
        }

        Some(Self {
            min_x,
            min_z,
            max_x,
            max_z,
        })
    }

    fn intersects(&self, other: &Bounds) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_z <= other.max_z
            && self.max_z >= other.min_z
    }

    fn clamp_to_bbox(self, bbox: &XZBBox) -> Option<Self> {
        let min_x = self.min_x.max(bbox.min_x());
        let max_x = self.max_x.min(bbox.max_x());
        let min_z = self.min_z.max(bbox.min_z());
        let max_z = self.max_z.min(bbox.max_z());

        if min_x > max_x || min_z > max_z {
            return None;
        }

        Some(Self {
            min_x,
            min_z,
            max_x,
            max_z,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WayRef<'a> {
    pub id: u64,
    pub tags: &'a HashMap<String, String>,
}

pub struct ElementContext<'a> {
    way_tagged_nodes: HashMap<u64, Vec<&'a ProcessedNode>>,
    node_parent_ways: HashMap<u64, Vec<WayRef<'a>>>,
    area_contained_elements: HashMap<u64, Vec<&'a ProcessedElement>>,
}

impl<'a> ElementContext<'a> {
    pub fn new(elements: &'a [ProcessedElement], xzbbox: &XZBBox) -> Self {
        let tagged_node_ids = collect_tagged_node_ids(elements);

        let mut way_tagged_nodes: HashMap<u64, Vec<&'a ProcessedNode>> = HashMap::new();
        let mut node_parent_ways: HashMap<u64, Vec<WayRef<'a>>> = HashMap::new();
        let mut areas: Vec<AreaCandidate<'a>> = Vec::new();

        for element in elements {
            let ProcessedElement::Way(way) = element else {
                continue;
            };

            let tagged_nodes: Vec<&ProcessedNode> = way
                .nodes
                .iter()
                .filter(|node| !node.tags.is_empty())
                .collect();
            if !tagged_nodes.is_empty() {
                way_tagged_nodes.insert(way.id, tagged_nodes);
            }

            let way_ref = WayRef {
                id: way.id,
                tags: &way.tags,
            };
            let mut first_id: Option<u64> = None;
            for (idx, node) in way.nodes.iter().enumerate() {
                if idx == 0 {
                    first_id = Some(node.id);
                }
                if idx + 1 == way.nodes.len() && first_id == Some(node.id) {
                    continue;
                }
                if !tagged_node_ids.contains(&node.id) {
                    continue;
                }
                node_parent_ways
                    .entry(node.id)
                    .or_default()
                    .push(way_ref);
            }

            if is_closed_way(way) && !way.tags.is_empty() {
                if let Some(bounds) = Bounds::from_nodes(&way.nodes) {
                    areas.push(AreaCandidate {
                        id: way.id,
                        nodes: &way.nodes,
                        bounds,
                    });
                }
            }
        }

        let cell_size = choose_cell_size(xzbbox);
        let mut spatial_index = SpatialIndex::new(xzbbox, cell_size);
        let mut element_bounds: Vec<Option<Bounds>> = vec![None; elements.len()];

        for (idx, element) in elements.iter().enumerate() {
            if element.tags().is_empty() {
                continue;
            }
            let Some(bounds) = bounds_for_element(element) else {
                continue;
            };
            let Some(clamped) = bounds.clamp_to_bbox(xzbbox) else {
                continue;
            };
            element_bounds[idx] = Some(clamped);
            spatial_index.insert(idx, &clamped);
        }

        let area_contained_elements =
            build_area_containment(elements, &areas, &spatial_index, &element_bounds);

        Self {
            way_tagged_nodes,
            node_parent_ways,
            area_contained_elements,
        }
    }

    pub fn tagged_nodes_on_way(&self, way_id: u64) -> Option<&[&'a ProcessedNode]> {
        self.way_tagged_nodes
            .get(&way_id)
            .map(|nodes| nodes.as_slice())
    }

    pub fn parent_ways_for_node(&self, node_id: u64) -> Option<&[WayRef<'a>]> {
        self.node_parent_ways
            .get(&node_id)
            .map(|ways| ways.as_slice())
    }

    pub fn contained_elements_for_area(
        &self,
        way_id: u64,
    ) -> Option<&[&'a ProcessedElement]> {
        self.area_contained_elements
            .get(&way_id)
            .map(|elements| elements.as_slice())
    }
}

struct AreaCandidate<'a> {
    id: u64,
    nodes: &'a [ProcessedNode],
    bounds: Bounds,
}

struct SpatialIndex {
    cell_size: i32,
    origin_x: i32,
    origin_z: i32,
    cells: HashMap<(i32, i32), Vec<usize>>,
}

impl SpatialIndex {
    fn new(bbox: &XZBBox, cell_size: i32) -> Self {
        Self {
            cell_size,
            origin_x: bbox.min_x(),
            origin_z: bbox.min_z(),
            cells: HashMap::new(),
        }
    }

    fn insert(&mut self, index: usize, bounds: &Bounds) {
        let (min_x, max_x, min_z, max_z) = self.cell_range(bounds);
        for cell_x in min_x..=max_x {
            for cell_z in min_z..=max_z {
                self.cells
                    .entry((cell_x, cell_z))
                    .or_default()
                    .push(index);
            }
        }
    }

    fn query(&self, bounds: &Bounds, seen: &mut [bool], out: &mut Vec<usize>) {
        let (min_x, max_x, min_z, max_z) = self.cell_range(bounds);
        for cell_x in min_x..=max_x {
            for cell_z in min_z..=max_z {
                let Some(entries) = self.cells.get(&(cell_x, cell_z)) else {
                    continue;
                };
                for &index in entries {
                    if seen[index] {
                        continue;
                    }
                    seen[index] = true;
                    out.push(index);
                }
            }
        }
    }

    fn cell_range(&self, bounds: &Bounds) -> (i32, i32, i32, i32) {
        let min_x = (bounds.min_x - self.origin_x) / self.cell_size;
        let max_x = (bounds.max_x - self.origin_x) / self.cell_size;
        let min_z = (bounds.min_z - self.origin_z) / self.cell_size;
        let max_z = (bounds.max_z - self.origin_z) / self.cell_size;
        (min_x, max_x, min_z, max_z)
    }
}

fn collect_tagged_node_ids(elements: &[ProcessedElement]) -> HashSet<u64> {
    let mut tagged_node_ids: HashSet<u64> = HashSet::new();
    for element in elements {
        if let ProcessedElement::Node(node) = element {
            tagged_node_ids.insert(node.id);
        }
    }
    tagged_node_ids
}

fn choose_cell_size(xzbbox: &XZBBox) -> i32 {
    let width = (xzbbox.max_x() - xzbbox.min_x()).abs().max(1);
    let height = (xzbbox.max_z() - xzbbox.min_z()).abs().max(1);
    let max_dim = width.max(height);
    let mut size = max_dim / 64;
    if size < 32 {
        size = 32;
    }
    if size > 256 {
        size = 256;
    }
    size
}

fn is_closed_way(way: &ProcessedWay) -> bool {
    if way.nodes.len() < 3 {
        return false;
    }
    let first = &way.nodes[0];
    let last = &way.nodes[way.nodes.len() - 1];
    first.id == last.id || (first.x == last.x && first.z == last.z)
}

fn bounds_for_element(element: &ProcessedElement) -> Option<Bounds> {
    match element {
        ProcessedElement::Node(node) => Some(Bounds {
            min_x: node.x,
            min_z: node.z,
            max_x: node.x,
            max_z: node.z,
        }),
        ProcessedElement::Way(way) => Bounds::from_nodes(&way.nodes),
        ProcessedElement::Relation(_) => None,
    }
}

fn build_area_containment<'a>(
    elements: &'a [ProcessedElement],
    areas: &[AreaCandidate<'a>],
    spatial_index: &SpatialIndex,
    element_bounds: &[Option<Bounds>],
) -> HashMap<u64, Vec<&'a ProcessedElement>> {
    let mut area_contained_elements: HashMap<u64, Vec<&'a ProcessedElement>> = HashMap::new();
    if areas.is_empty() {
        return area_contained_elements;
    }

    let mut seen = vec![false; elements.len()];
    let mut candidates: Vec<usize> = Vec::new();

    for area in areas {
        candidates.clear();
        spatial_index.query(&area.bounds, &mut seen, &mut candidates);

        let mut contained: Vec<&'a ProcessedElement> = Vec::new();
        for &index in &candidates {
            let element = &elements[index];
            if let ProcessedElement::Way(way) = element {
                if way.id == area.id {
                    continue;
                }
            }

            let Some(bounds) = element_bounds[index] else {
                continue;
            };
            if !bounds.intersects(&area.bounds) {
                continue;
            }

            if element_inside_area(element, area.nodes, &area.bounds) {
                contained.push(element);
            }
        }

        for &index in &candidates {
            seen[index] = false;
        }

        if !contained.is_empty() {
            area_contained_elements.insert(area.id, contained);
        }
    }

    area_contained_elements
}

fn element_inside_area(
    element: &ProcessedElement,
    area_nodes: &[ProcessedNode],
    area_bounds: &Bounds,
) -> bool {
    match element {
        ProcessedElement::Node(node) => point_in_polygon(node.x, node.z, area_nodes, area_bounds),
        ProcessedElement::Way(way) => way
            .nodes
            .iter()
            .any(|node| point_in_polygon(node.x, node.z, area_nodes, area_bounds)),
        ProcessedElement::Relation(_) => false,
    }
}

fn point_in_polygon(px: i32, pz: i32, polygon: &[ProcessedNode], bounds: &Bounds) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    if px < bounds.min_x || px > bounds.max_x || pz < bounds.min_z || pz > bounds.max_z {
        return false;
    }

    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let xi = polygon[i].x;
        let zi = polygon[i].z;
        let xj = polygon[j].x;
        let zj = polygon[j].z;

        if point_on_segment(px, pz, xi, zi, xj, zj) {
            return true;
        }

        let intersect = ((zi > pz) != (zj > pz))
            && ((px as f64)
                < (xj - xi) as f64 * (pz - zi) as f64 / (zj - zi) as f64 + xi as f64);
        if intersect {
            inside = !inside;
        }
        j = i;
    }

    inside
}

fn point_on_segment(px: i32, pz: i32, x1: i32, z1: i32, x2: i32, z2: i32) -> bool {
    let cross = (px - x1) as i64 * (z2 - z1) as i64
        - (pz - z1) as i64 * (x2 - x1) as i64;
    if cross != 0 {
        return false;
    }

    let min_x = x1.min(x2);
    let max_x = x1.max(x2);
    let min_z = z1.min(z2);
    let max_z = z1.max(z2);

    px >= min_x && px <= max_x && pz >= min_z && pz <= max_z
}
