use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Right,
    Down,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PanePlan {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LayoutNode {
    Pane {
        #[serde(flatten)]
        pane: PanePlan,
    },
    Split {
        direction: Direction,
        ratio: f64,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    pub fn pane(pane: PanePlan) -> Self {
        Self::Pane { pane }
    }

    pub fn pane_count(&self) -> usize {
        match self {
            Self::Pane { .. } => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }
}

pub fn preset(name: Option<&str>, panes: Vec<PanePlan>) -> Result<LayoutNode> {
    if panes.is_empty() {
        return Err(Error::InvalidConfig(
            "a window must contain at least one pane".into(),
        ));
    }
    if panes.len() == 1 {
        return Ok(LayoutNode::pane(panes.into_iter().next().unwrap()));
    }

    match name.unwrap_or("even-horizontal") {
        "even-horizontal" => Ok(even(Direction::Right, panes)),
        "even-vertical" => Ok(even(Direction::Down, panes)),
        "main-vertical" => Ok(main_layout(Direction::Right, Direction::Down, panes)),
        "main-horizontal" => Ok(main_layout(Direction::Down, Direction::Right, panes)),
        "tiled" => Ok(tiled(panes)),
        other => Err(Error::InvalidConfig(format!(
            "unsupported layout '{other}'; expected main-vertical, main-horizontal, tiled, even-horizontal, or even-vertical"
        ))),
    }
}

fn split(direction: Direction, ratio: f64, first: LayoutNode, second: LayoutNode) -> LayoutNode {
    LayoutNode::Split {
        direction,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    }
}

fn even(direction: Direction, mut panes: Vec<PanePlan>) -> LayoutNode {
    if panes.len() == 1 {
        return LayoutNode::pane(panes.remove(0));
    }
    let count = panes.len();
    let first = LayoutNode::pane(panes.remove(0));
    split(direction, 1.0 / count as f64, first, even(direction, panes))
}

fn main_layout(
    main_direction: Direction,
    secondary_direction: Direction,
    mut panes: Vec<PanePlan>,
) -> LayoutNode {
    let main = LayoutNode::pane(panes.remove(0));
    split(main_direction, 0.6, main, even(secondary_direction, panes))
}

fn tiled(panes: Vec<PanePlan>) -> LayoutNode {
    let columns = (panes.len() as f64).sqrt().ceil() as usize;
    let rows = panes.len().div_ceil(columns);
    let mut iter = panes.into_iter();
    let mut groups = Vec::new();

    for column in 0..columns {
        let remaining = iter.len();
        if remaining == 0 {
            break;
        }
        let columns_left = columns - column;
        let size = remaining.div_ceil(columns_left).min(rows);
        let group: Vec<_> = iter.by_ref().take(size).collect();
        groups.push(even(Direction::Down, group));
    }

    even_nodes(Direction::Right, groups)
}

fn even_nodes(direction: Direction, mut nodes: Vec<LayoutNode>) -> LayoutNode {
    if nodes.len() == 1 {
        return nodes.remove(0);
    }
    let count = nodes.len();
    let first = nodes.remove(0);
    split(
        direction,
        1.0 / count as f64,
        first,
        even_nodes(direction, nodes),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panes(count: usize) -> Vec<PanePlan> {
        (0..count)
            .map(|index| PanePlan {
                name: Some(format!("p{index}")),
                commands: vec![],
            })
            .collect()
    }

    #[test]
    fn all_presets_keep_every_pane() {
        for layout in [
            "main-vertical",
            "main-horizontal",
            "tiled",
            "even-horizontal",
            "even-vertical",
        ] {
            assert_eq!(preset(Some(layout), panes(5)).unwrap().pane_count(), 5);
        }
    }

    #[test]
    fn unknown_layout_is_rejected() {
        assert!(preset(Some("spiral"), panes(2)).is_err());
    }
}
