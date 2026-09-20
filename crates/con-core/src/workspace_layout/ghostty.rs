use std::collections::{HashMap, HashSet};

use super::*;

#[derive(Default)]
struct NodeDef {
    kind: Option<String>,
    direction: Option<String>,
    ratio: Option<f32>,
    first: Option<String>,
    second: Option<String>,
    pane: Option<String>,
}

#[derive(Default)]
struct TabBuild {
    tab: Option<WorkspaceTab>,
    nodes: HashMap<String, NodeDef>,
    root_node: Option<String>,
}

/// Serialize in Ghostty's readable `key = value` style. Repeated `tab`, `pane`,
/// and `surface` declarations define their order; node references define the tree.
pub(super) fn serialize(layout: &WorkspaceLayout) -> anyhow::Result<String> {
    layout.validate()?;
    let mut out = String::from("# Con workspace layout. Declaration order is display order.\n");
    line(&mut out, "format", &quote(&layout.format)?);
    line(&mut out, "version", &layout.version.to_string());
    optional(&mut out, "name", layout.name.as_deref())?;
    line(&mut out, "root", &quote(&layout.root)?);
    optional(&mut out, "active-tab", layout.active_tab.as_deref())?;
    optional(&mut out, "default.shell", layout.defaults.shell.as_deref())?;
    optional(
        &mut out,
        "default.agent-provider",
        layout.defaults.agent_provider.as_deref(),
    )?;
    optional(
        &mut out,
        "default.agent-model",
        layout.defaults.agent_model.as_deref(),
    )?;

    for tab in &layout.tabs {
        out.push('\n');
        line(&mut out, "tab", &quote(&tab.id)?);
        optional(&mut out, "tab.title", tab.title.as_deref())?;
        optional(&mut out, "tab.cwd", tab.cwd.as_deref())?;
        optional(&mut out, "tab.active-pane", tab.active_pane.as_deref())?;
        optional(
            &mut out,
            "tab.agent-provider",
            tab.agent.provider.as_deref(),
        )?;
        optional(&mut out, "tab.agent-model", tab.agent.model.as_deref())?;
        if tab.layout.is_some() {
            line(&mut out, "tab.layout", &quote("node-1")?);
        }
        for pane in &tab.panes {
            line(&mut out, "pane", &quote(&pane.id)?);
            optional(&mut out, "pane.title", pane.title.as_deref())?;
            optional(&mut out, "pane.cwd", pane.cwd.as_deref())?;
            optional(
                &mut out,
                "pane.active-surface",
                pane.active_surface.as_deref(),
            )?;
            for surface in &pane.surfaces {
                line(&mut out, "surface", &quote(&surface.id)?);
                optional(&mut out, "surface.title", surface.title.as_deref())?;
                optional(&mut out, "surface.owner", surface.owner.as_deref())?;
                optional(&mut out, "surface.cwd", surface.cwd.as_deref())?;
                line(
                    &mut out,
                    "surface.close-pane-when-last",
                    if surface.close_pane_when_last {
                        "true"
                    } else {
                        "false"
                    },
                );
            }
        }
        let mut next = 1;
        if let Some(root) = &tab.layout {
            write_node(&mut out, root, &mut next)?;
        }
    }
    Ok(out)
}

fn write_node(
    out: &mut String,
    node: &WorkspaceLayoutNode,
    next: &mut usize,
) -> anyhow::Result<String> {
    let id = format!("node-{}", *next);
    *next += 1;
    match node {
        WorkspaceLayoutNode::Pane { id: pane } => {
            line(out, "node", &quote(&id)?);
            line(out, "node.kind", "pane");
            line(out, "node.pane", &quote(pane)?);
        }
        WorkspaceLayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            // Definitions may precede their parent: references, not declaration
            // order, determine the tree. Serialize each subtree exactly once.
            let first_id = write_node(out, first, next)?;
            let second_id = write_node(out, second, next)?;
            line(out, "node", &quote(&id)?);
            line(out, "node.kind", "split");
            line(
                out,
                "node.direction",
                match direction {
                    WorkspaceSplitDirection::Horizontal => "horizontal",
                    WorkspaceSplitDirection::Vertical => "vertical",
                },
            );
            line(out, "node.ratio", &ratio.to_string());
            line(out, "node.first", &quote(&first_id)?);
            line(out, "node.second", &quote(&second_id)?);
        }
    }
    Ok(id)
}

fn line(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(" = ");
    out.push_str(value);
    out.push('\n');
}
fn optional(out: &mut String, key: &str, value: Option<&str>) -> anyhow::Result<()> {
    if let Some(value) = value {
        line(out, key, &quote(value)?);
    }
    Ok(())
}
fn quote(value: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        !value.contains(['\n', '\r', '\0']),
        "workspace strings cannot contain newlines or NUL"
    );
    Ok(format!("\"{value}\""))
}
fn value(raw: &str) -> anyhow::Result<String> {
    let raw = raw.trim();
    anyhow::ensure!(
        raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"'),
        "string values require Ghostty-compatible outer quotes"
    );
    Ok(raw[1..raw.len() - 1].to_string())
}

pub(super) fn parse(input: &str) -> anyhow::Result<WorkspaceLayout> {
    let mut layout = WorkspaceLayout::default();
    layout.tabs.clear();
    let mut builds: Vec<TabBuild> = Vec::new();
    let mut pane_index = None;
    let mut surface_index = None;
    let mut node_id: Option<String> = None;
    let mut seen_single = HashSet::new();

    for (index, raw) in input.lines().enumerate() {
        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let (key, raw_value) = raw
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("line {}: expected key = value", index + 1))?;
        let key = key.trim();
        let raw_value = raw_value.trim();
        let unique = |seen: &mut HashSet<String>, key: &str| -> anyhow::Result<()> {
            anyhow::ensure!(
                seen.insert(key.to_string()),
                "line {}: duplicate key {key}",
                index + 1
            );
            Ok(())
        };
        match key {
            "format" => {
                unique(&mut seen_single, key)?;
                layout.format = value(raw_value)?;
            }
            "version" => {
                unique(&mut seen_single, key)?;
                layout.version = raw_value.parse()?;
            }
            "name" => {
                unique(&mut seen_single, key)?;
                layout.name = Some(value(raw_value)?);
            }
            "root" => {
                unique(&mut seen_single, key)?;
                layout.root = value(raw_value)?;
            }
            "active-tab" => {
                unique(&mut seen_single, key)?;
                layout.active_tab = Some(value(raw_value)?);
            }
            "default.shell" => layout.defaults.shell = Some(value(raw_value)?),
            "default.agent-provider" => layout.defaults.agent_provider = Some(value(raw_value)?),
            "default.agent-model" => layout.defaults.agent_model = Some(value(raw_value)?),
            "tab" => {
                builds.push(TabBuild {
                    tab: Some(WorkspaceTab {
                        id: value(raw_value)?,
                        title: None,
                        cwd: None,
                        active_pane: None,
                        agent: WorkspaceTabAgent::default(),
                        layout: None,
                        panes: vec![],
                    }),
                    ..Default::default()
                });
                pane_index = None;
                surface_index = None;
                node_id = None;
            }
            "tab.title" => current_tab(&mut builds)?.title = Some(value(raw_value)?),
            "tab.cwd" => current_tab(&mut builds)?.cwd = Some(value(raw_value)?),
            "tab.active-pane" => current_tab(&mut builds)?.active_pane = Some(value(raw_value)?),
            "tab.agent-provider" => {
                current_tab(&mut builds)?.agent.provider = Some(value(raw_value)?)
            }
            "tab.agent-model" => current_tab(&mut builds)?.agent.model = Some(value(raw_value)?),
            "tab.layout" => {
                builds
                    .last_mut()
                    .ok_or_else(|| anyhow::anyhow!("tab.layout before tab"))?
                    .root_node = Some(value(raw_value)?)
            }
            "pane" => {
                let tab = current_tab(&mut builds)?;
                tab.panes.push(WorkspacePane {
                    id: value(raw_value)?,
                    title: None,
                    cwd: None,
                    active_surface: None,
                    surfaces: vec![],
                });
                pane_index = Some(tab.panes.len() - 1);
                surface_index = None;
            }
            "pane.title" => current_pane(&mut builds, pane_index)?.title = Some(value(raw_value)?),
            "pane.cwd" => current_pane(&mut builds, pane_index)?.cwd = Some(value(raw_value)?),
            "pane.active-surface" => {
                current_pane(&mut builds, pane_index)?.active_surface = Some(value(raw_value)?)
            }
            "surface" => {
                let pane = current_pane(&mut builds, pane_index)?;
                pane.surfaces.push(WorkspaceSurface {
                    id: value(raw_value)?,
                    title: None,
                    owner: None,
                    cwd: None,
                    close_pane_when_last: false,
                });
                surface_index = Some(pane.surfaces.len() - 1);
            }
            "surface.title" => {
                current_surface(&mut builds, pane_index, surface_index)?.title =
                    Some(value(raw_value)?)
            }
            "surface.owner" => {
                current_surface(&mut builds, pane_index, surface_index)?.owner =
                    Some(value(raw_value)?)
            }
            "surface.cwd" => {
                current_surface(&mut builds, pane_index, surface_index)?.cwd =
                    Some(value(raw_value)?)
            }
            "surface.close-pane-when-last" => {
                current_surface(&mut builds, pane_index, surface_index)?.close_pane_when_last =
                    parse_bool(raw_value)?
            }
            "node" => {
                let id = value(raw_value)?;
                let build = builds
                    .last_mut()
                    .ok_or_else(|| anyhow::anyhow!("node before tab"))?;
                anyhow::ensure!(!build.nodes.contains_key(&id), "duplicate node id {id:?}");
                build.nodes.insert(id.clone(), NodeDef::default());
                node_id = Some(id);
            }
            "node.kind" => current_node(&mut builds, &node_id)?.kind = Some(raw_value.to_string()),
            "node.direction" => {
                current_node(&mut builds, &node_id)?.direction = Some(raw_value.to_string())
            }
            "node.ratio" => current_node(&mut builds, &node_id)?.ratio = Some(raw_value.parse()?),
            "node.first" => current_node(&mut builds, &node_id)?.first = Some(value(raw_value)?),
            "node.second" => current_node(&mut builds, &node_id)?.second = Some(value(raw_value)?),
            "node.pane" => current_node(&mut builds, &node_id)?.pane = Some(value(raw_value)?),
            _ => anyhow::bail!("line {}: unknown workspace key {key:?}", index + 1),
        }
    }
    for mut build in builds {
        let mut tab = build.tab.take().unwrap();
        let mut consumed = HashSet::new();
        if let Some(root) = build.root_node.as_deref() {
            tab.layout = Some(build_node(root, &build.nodes, &mut consumed, 0)?);
        }
        anyhow::ensure!(
            consumed.len() == build.nodes.len(),
            "unreachable layout nodes"
        );
        layout.tabs.push(tab);
    }
    for required in ["format", "version", "root"] {
        anyhow::ensure!(
            seen_single.contains(required),
            "missing required key {required}"
        );
    }
    layout.validate()?;
    Ok(layout)
}

fn current_tab(builds: &mut [TabBuild]) -> anyhow::Result<&mut WorkspaceTab> {
    builds
        .last_mut()
        .and_then(|b| b.tab.as_mut())
        .ok_or_else(|| anyhow::anyhow!("tab field before tab declaration"))
}
fn current_pane(
    builds: &mut [TabBuild],
    index: Option<usize>,
) -> anyhow::Result<&mut WorkspacePane> {
    let i = index.ok_or_else(|| anyhow::anyhow!("pane field before pane declaration"))?;
    current_tab(builds)?
        .panes
        .get_mut(i)
        .ok_or_else(|| anyhow::anyhow!("invalid pane context"))
}
fn current_surface(
    builds: &mut [TabBuild],
    pane: Option<usize>,
    surface: Option<usize>,
) -> anyhow::Result<&mut WorkspaceSurface> {
    let i = surface.ok_or_else(|| anyhow::anyhow!("surface field before surface declaration"))?;
    current_pane(builds, pane)?
        .surfaces
        .get_mut(i)
        .ok_or_else(|| anyhow::anyhow!("invalid surface context"))
}
fn current_node<'a>(
    builds: &'a mut [TabBuild],
    id: &Option<String>,
) -> anyhow::Result<&'a mut NodeDef> {
    let id = id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("node field before node declaration"))?;
    builds
        .last_mut()
        .and_then(|b| b.nodes.get_mut(id))
        .ok_or_else(|| anyhow::anyhow!("invalid node context"))
}
fn parse_bool(raw: &str) -> anyhow::Result<bool> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => anyhow::bail!("invalid boolean {raw:?}"),
    }
}

fn build_node(
    id: &str,
    nodes: &HashMap<String, NodeDef>,
    visiting: &mut HashSet<String>,
    depth: usize,
) -> anyhow::Result<WorkspaceLayoutNode> {
    anyhow::ensure!(depth < 64, "layout exceeds maximum depth of 64");
    anyhow::ensure!(
        visiting.insert(id.to_string()),
        "cycle or shared layout node at {id:?}"
    );
    let node = nodes
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("dangling layout node reference {id:?}"))?;
    let result = match node.kind.as_deref() {
        Some("pane") => WorkspaceLayoutNode::Pane {
            id: node
                .pane
                .clone()
                .ok_or_else(|| anyhow::anyhow!("pane node {id:?} has no pane reference"))?,
        },
        Some("split") => WorkspaceLayoutNode::Split {
            direction: match node.direction.as_deref() {
                Some("horizontal") => WorkspaceSplitDirection::Horizontal,
                Some("vertical") => WorkspaceSplitDirection::Vertical,
                _ => anyhow::bail!("invalid direction in node {id:?}"),
            },
            ratio: {
                let ratio = node
                    .ratio
                    .ok_or_else(|| anyhow::anyhow!("split node {id:?} has no ratio"))?;
                anyhow::ensure!(
                    ratio.is_finite() && ratio > 0.0 && ratio < 1.0,
                    "invalid split ratio"
                );
                ratio
            },
            first: Box::new(build_node(
                node.first
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("split node {id:?} has no first reference"))?,
                nodes,
                visiting,
                depth + 1,
            )?),
            second: Box::new(build_node(
                node.second
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("split node {id:?} has no second reference"))?,
                nodes,
                visiting,
                depth + 1,
            )?),
        },
        _ => anyhow::bail!("invalid kind in node {id:?}"),
    };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "format = \"con.workspace.layout\"\nversion = 2\nroot = \".\"\n";

    #[test]
    fn rejects_shared_nodes_unreachable_nodes_and_excessive_depth() {
        let shared = format!(
            "{HEADER}tab = \"t\"\ntab.layout = \"n\"\nnode = \"n\"\nnode.kind = split\nnode.direction = horizontal\nnode.ratio = 0.4\nnode.first = \"leaf\"\nnode.second = \"leaf\"\nnode = \"leaf\"\nnode.kind = pane\nnode.pane = \"p\"\n"
        );
        assert!(parse(&shared).unwrap_err().to_string().contains("shared"));
        let unreachable = format!(
            "{HEADER}tab = \"t\"\nnode = \"unused\"\nnode.kind = pane\nnode.pane = \"p\"\n"
        );
        assert!(
            parse(&unreachable)
                .unwrap_err()
                .to_string()
                .contains("unreachable")
        );
        let mut deep = format!("{HEADER}tab = \"t\"\ntab.layout = \"n0\"\n");
        for index in 0..65 {
            deep.push_str(&format!("node = \"n{index}\"\nnode.kind = split\nnode.direction = vertical\nnode.ratio = 0.3\nnode.first = \"n{}\"\nnode.second = \"unused{index}\"\n", index + 1));
        }
        assert!(parse(&deep).unwrap_err().to_string().contains("depth"));
    }

    #[test]
    fn rejects_duplicate_ids_invalid_ratio_dangling_refs_and_cycles() {
        let duplicate = format!("{HEADER}tab = \"t\"\npane = \"p\"\npane = \"p\"\n");
        assert!(
            parse(&duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate pane")
        );

        let ratio = format!(
            "{HEADER}tab = \"t\"\ntab.layout = \"n\"\npane = \"p\"\nnode = \"n\"\nnode.kind = split\nnode.direction = horizontal\nnode.ratio = 1.2\nnode.first = \"a\"\nnode.second = \"a\"\nnode = \"a\"\nnode.kind = pane\nnode.pane = \"p\"\n"
        );
        assert!(parse(&ratio).unwrap_err().to_string().contains("ratio"));

        let dangling = format!("{HEADER}tab = \"t\"\ntab.layout = \"missing\"\n");
        assert!(
            parse(&dangling)
                .unwrap_err()
                .to_string()
                .contains("dangling")
        );

        let cycle = format!(
            "{HEADER}tab = \"t\"\ntab.layout = \"n\"\nnode = \"n\"\nnode.kind = split\nnode.direction = vertical\nnode.ratio = 0.5\nnode.first = \"n\"\nnode.second = \"n\"\n"
        );
        assert!(parse(&cycle).unwrap_err().to_string().contains("cycle"));
    }
}
