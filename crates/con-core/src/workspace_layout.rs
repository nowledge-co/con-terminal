use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::session::{
    AgentModelOverrideState, AgentRoutingState, CommandHistoryEntryState, GlobalHistoryState,
    PaneLayoutState, PaneSplitDirection, PaneState, Session, SurfaceState, TabState,
};
use con_agent::ProviderKind;

mod ghostty;

pub const WORKSPACE_LAYOUT_VERSION: u32 = 2;
pub const WORKSPACE_LAYOUT_FORMAT: &str = "con.workspace.layout";
pub const DEFAULT_WORKSPACE_LAYOUT_PATH: &str = ".con/workspace.ghostty";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceLayout {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_root")]
    pub root: String,
    #[serde(default)]
    pub active_tab: Option<String>,
    #[serde(default)]
    pub defaults: WorkspaceDefaults,
    #[serde(default)]
    pub tabs: Vec<WorkspaceTab>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceDefaults {
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub agent_provider: Option<String>,
    #[serde(default)]
    pub agent_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceTab {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub active_pane: Option<String>,
    #[serde(default)]
    pub agent: WorkspaceTabAgent,
    #[serde(default)]
    pub layout: Option<WorkspaceLayoutNode>,
    #[serde(default)]
    pub panes: Vec<WorkspacePane>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceTabAgent {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceLayoutNode {
    Pane {
        id: String,
    },
    Split {
        direction: WorkspaceSplitDirection,
        ratio: f32,
        first: Box<WorkspaceLayoutNode>,
        second: Box<WorkspaceLayoutNode>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspacePane {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub active_surface: Option<String>,
    #[serde(default)]
    pub surfaces: Vec<WorkspaceSurface>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceSurface {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub close_pane_when_last: bool,
}

fn default_format() -> String {
    WORKSPACE_LAYOUT_FORMAT.to_string()
}

fn default_version() -> u32 {
    WORKSPACE_LAYOUT_VERSION
}

fn default_root() -> String {
    ".".to_string()
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            format: default_format(),
            version: WORKSPACE_LAYOUT_VERSION,
            name: None,
            root: default_root(),
            active_tab: None,
            defaults: WorkspaceDefaults::default(),
            tabs: Vec::new(),
        }
    }
}

impl WorkspaceLayout {
    pub fn from_toml_str(input: &str) -> anyhow::Result<Self> {
        let mut layout: Self = toml::from_str(input)?;
        anyhow::ensure!(
            layout.version == 1,
            "unsupported legacy workspace layout version {}",
            layout.version
        );
        layout.version = WORKSPACE_LAYOUT_VERSION;
        layout.validate()?;
        Ok(layout)
    }

    pub fn from_ghostty_str(input: &str) -> anyhow::Result<Self> {
        ghostty::parse(input)
    }

    pub fn to_ghostty_string(&self) -> anyhow::Result<String> {
        ghostty::serialize(self)
    }

    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)?;
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            Self::from_toml_str(&content)
        } else {
            Self::from_ghostty_str(&content)
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = path.as_ref();
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            anyhow::bail!("TOML workspace layouts are read-only; save as workspace.ghostty");
        }
        crate::config::write_private_atomic(path, self.to_ghostty_string()?.as_bytes())
    }

    pub fn default_path_for_root(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(DEFAULT_WORKSPACE_LAYOUT_PATH)
    }

    /// Prefer the new document even when malformed; never silently fall back.
    /// Old project layouts are imported in memory, without changing the repo.
    pub fn existing_path_for_root(root: impl AsRef<Path>) -> PathBuf {
        let current = Self::default_path_for_root(&root);
        if current.exists() {
            current
        } else {
            root.as_ref().join(".con/workspace.toml")
        }
    }

    pub fn from_session(session: &Session, root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        let mut used_tab_ids = HashSet::new();
        let mut tabs = Vec::new();

        for tab in &session.tabs {
            let tab_name = tab
                .user_label
                .as_deref()
                .or_else(|| (!tab.title.trim().is_empty()).then_some(tab.title.as_str()))
                .unwrap_or("Tab");
            let tab_id = unique_id(&slug_id(tab_name, "tab"), &mut used_tab_ids);
            let mut used_pane_ids = HashSet::new();
            let mut pane_id_map = HashMap::new();
            let mut surface_id_map = HashMap::new();
            let mut panes = Vec::new();

            if let Some(layout) = tab.layout.as_ref() {
                collect_workspace_panes(
                    layout,
                    root,
                    &mut used_pane_ids,
                    &mut pane_id_map,
                    &mut surface_id_map,
                    &mut panes,
                );
            } else {
                for (pane_index, pane) in tab.panes.iter().enumerate() {
                    let pane_id =
                        unique_id(&format!("pane-{}", pane_index + 1), &mut used_pane_ids);
                    pane_id_map.insert(pane_index, pane_id.clone());
                    panes.push(WorkspacePane {
                        id: pane_id,
                        title: Some(format!("Pane {}", pane_index + 1)),
                        cwd: layout_cwd(pane.cwd.as_deref(), root),
                        active_surface: None,
                        surfaces: Vec::new(),
                    });
                }
            }

            let active_pane = tab
                .focused_pane_id
                .and_then(|pane_id| pane_id_map.get(&pane_id).cloned());
            let layout = tab
                .layout
                .as_ref()
                .and_then(|layout| workspace_node_from_session(layout, &pane_id_map))
                .or_else(|| workspace_node_from_panes(&panes));
            let agent = workspace_tab_agent_from_routing(&tab.agent_routing);

            tabs.push(WorkspaceTab {
                id: tab_id,
                title: tab.user_label.clone().or_else(|| Some(tab.title.clone())),
                cwd: layout_cwd(tab.cwd.as_deref(), root),
                active_pane,
                agent,
                layout,
                panes,
            });
        }

        let active_tab = tabs
            .get(session.active_tab.min(tabs.len().saturating_sub(1)))
            .map(|tab| tab.id.clone());

        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .map(ToOwned::to_owned);

        Self {
            format: WORKSPACE_LAYOUT_FORMAT.to_string(),
            version: WORKSPACE_LAYOUT_VERSION,
            name,
            root: ".".to_string(),
            active_tab,
            defaults: WorkspaceDefaults::default(),
            tabs,
        }
    }

    pub fn to_session(
        &self,
        root: impl AsRef<Path>,
        history: Option<&GlobalHistoryState>,
    ) -> anyhow::Result<Session> {
        self.validate()?;
        let root = root.as_ref();
        let default_provider = parse_provider_kind(self.defaults.agent_provider.as_deref());
        let default_model = self.defaults.agent_model.clone();

        let mut tabs = Vec::new();
        for tab in &self.tabs {
            let provider =
                parse_provider_kind(tab.agent.provider.as_deref()).or(default_provider.clone());
            let model = tab.agent.model.clone().or_else(|| default_model.clone());
            let pane_id_map = tab
                .panes
                .iter()
                .enumerate()
                .map(|(idx, pane)| (pane.id.as_str(), idx))
                .collect::<HashMap<_, _>>();
            let layout = match tab.layout.as_ref() {
                Some(node) => Some(session_node_from_workspace(node, tab, root, &pane_id_map)?),
                None if !tab.panes.is_empty() => Some(PaneLayoutState::Leaf {
                    pane_id: 0,
                    cwd: resolved_layout_cwd(
                        tab.panes[0].cwd.as_deref().or(tab.cwd.as_deref()),
                        root,
                    ),
                    active_surface_id: active_surface_index(&tab.panes[0]),
                    surfaces: session_surfaces_from_workspace(&tab.panes[0], root),
                }),
                None => None,
            };
            let focused_pane_id = tab
                .active_pane
                .as_deref()
                .and_then(|id| pane_id_map.get(id).copied())
                .or_else(|| (!tab.panes.is_empty()).then_some(0));
            let panes = if tab.panes.is_empty() {
                vec![PaneState {
                    cwd: resolved_layout_cwd(tab.cwd.as_deref(), root),
                }]
            } else {
                tab.panes
                    .iter()
                    .map(|pane| PaneState {
                        cwd: resolved_layout_cwd(pane.cwd.as_deref().or(tab.cwd.as_deref()), root),
                    })
                    .collect()
            };
            let agent_routing = agent_routing_from_workspace(provider, model);
            let shell_history = panes
                .iter()
                .enumerate()
                .map(|(pane_id, _pane)| crate::session::PaneCommandHistoryState {
                    pane_id: Some(pane_id),
                    entries: Vec::new(),
                })
                .collect();

            tabs.push(TabState {
                title: tab
                    .title
                    .clone()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| tab.id.clone()),
                cwd: resolved_layout_cwd(tab.cwd.as_deref(), root),
                layout,
                focused_pane_id,
                panes,
                shell_history,
                conversation_id: None,
                agent_routing,
                user_label: tab.title.clone(),
                color: None,
            });
        }

        if tabs.is_empty() {
            tabs.push(TabState {
                title: self.name.clone().unwrap_or_else(|| "Terminal".to_string()),
                cwd: Some(root.to_string_lossy().to_string()),
                layout: None,
                focused_pane_id: Some(0),
                panes: vec![PaneState {
                    cwd: Some(root.to_string_lossy().to_string()),
                }],
                shell_history: Vec::new(),
                conversation_id: None,
                agent_routing: agent_routing_from_workspace(
                    default_provider.clone(),
                    default_model,
                ),
                user_label: self.name.clone(),
                color: None,
            });
        }

        let active_tab = self
            .active_tab
            .as_deref()
            .and_then(|active| self.tabs.iter().position(|tab| tab.id == active))
            .unwrap_or(0)
            .min(tabs.len().saturating_sub(1));
        let mut session = Session {
            tabs,
            active_tab,
            agent_panel_open: false,
            agent_panel_width: None,
            input_bar_visible: true,
            global_shell_history: history
                .map(|history| history.global_shell_history.clone())
                .unwrap_or_default(),
            input_history: profile_input_history(history),
            conversation_id: None,
            left_panel_width: None,
            vertical_tabs_pinned: None,
            activity_slot: None,
            left_panel_open: None,
            editor_area_height: None,
        };

        if session.global_shell_history.is_empty() && !session.input_history.is_empty() {
            session.global_shell_history = session
                .input_history
                .iter()
                .map(|command| CommandHistoryEntryState {
                    command: command.clone(),
                    cwd: None,
                })
                .collect();
        }

        Ok(session)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.format == WORKSPACE_LAYOUT_FORMAT,
            "unsupported workspace layout format {:?}",
            self.format
        );
        anyhow::ensure!(
            self.version == WORKSPACE_LAYOUT_VERSION,
            "unsupported workspace layout version {}",
            self.version
        );

        let mut tab_ids = std::collections::HashSet::new();
        for tab in &self.tabs {
            anyhow::ensure!(
                !tab.id.trim().is_empty(),
                "workspace tab id cannot be empty"
            );
            anyhow::ensure!(
                tab_ids.insert(tab.id.as_str()),
                "duplicate workspace tab id {:?}",
                tab.id
            );

            let mut pane_ids = std::collections::HashSet::new();
            for pane in &tab.panes {
                anyhow::ensure!(
                    !pane.id.trim().is_empty(),
                    "workspace pane id cannot be empty in tab {:?}",
                    tab.id
                );
                anyhow::ensure!(
                    pane_ids.insert(pane.id.as_str()),
                    "duplicate pane id {:?} in tab {:?}",
                    pane.id,
                    tab.id
                );

                let mut surface_ids = std::collections::HashSet::new();
                for surface in &pane.surfaces {
                    anyhow::ensure!(
                        !surface.id.trim().is_empty(),
                        "workspace surface id cannot be empty in pane {:?}",
                        pane.id
                    );
                    anyhow::ensure!(
                        surface_ids.insert(surface.id.as_str()),
                        "duplicate surface id {:?} in pane {:?}",
                        surface.id,
                        pane.id
                    );
                }

                if let Some(active_surface) = pane.active_surface.as_deref() {
                    anyhow::ensure!(
                        surface_ids.contains(active_surface),
                        "active surface {:?} is not defined in pane {:?}",
                        active_surface,
                        pane.id
                    );
                }
            }

            if let Some(active_pane) = tab.active_pane.as_deref() {
                anyhow::ensure!(
                    pane_ids.contains(active_pane),
                    "active pane {:?} is not defined in tab {:?}",
                    active_pane,
                    tab.id
                );
            }

            if let Some(layout) = tab.layout.as_ref() {
                let mut placed_panes = std::collections::HashSet::new();
                validate_layout_node(layout, &pane_ids, &mut placed_panes, &tab.id)?;
                anyhow::ensure!(
                    placed_panes.len() == pane_ids.len()
                        && pane_ids
                            .iter()
                            .all(|pane_id| placed_panes.contains(*pane_id)),
                    "layout in tab {:?} must reference each pane exactly once",
                    tab.id
                );
            } else {
                anyhow::ensure!(
                    tab.panes.len() <= 1,
                    "tab {:?} with multiple panes must define layout",
                    tab.id
                );
            }
        }

        if let Some(active_tab) = self.active_tab.as_deref() {
            anyhow::ensure!(
                self.tabs.iter().any(|tab| tab.id == active_tab),
                "active tab {:?} is not defined",
                active_tab
            );
        }

        Ok(())
    }
}

fn collect_workspace_panes(
    node: &PaneLayoutState,
    root: &Path,
    used_pane_ids: &mut HashSet<String>,
    pane_id_map: &mut HashMap<usize, String>,
    surface_id_map: &mut HashMap<(usize, usize), String>,
    panes: &mut Vec<WorkspacePane>,
) {
    match node {
        PaneLayoutState::Leaf {
            pane_id,
            cwd,
            active_surface_id,
            surfaces,
        } => {
            let fallback_id = format!("pane-{}", panes.len() + 1);
            let title_hint = surfaces
                .iter()
                .find_map(|surface| surface.title.as_deref())
                .or_else(|| surfaces.iter().find_map(|surface| surface.owner.as_deref()));
            let pane_id_text = unique_id(
                &slug_id(title_hint.unwrap_or(&fallback_id), &fallback_id),
                used_pane_ids,
            );
            pane_id_map.insert(*pane_id, pane_id_text.clone());

            let mut used_surface_ids = HashSet::new();
            let mut workspace_surfaces = Vec::new();
            for (surface_index, surface) in surfaces.iter().enumerate() {
                let fallback_surface_id = format!("surface-{}", surface_index + 1);
                let surface_id = unique_id(
                    &slug_id(
                        surface
                            .title
                            .as_deref()
                            .or(surface.owner.as_deref())
                            .unwrap_or(&fallback_surface_id),
                        &fallback_surface_id,
                    ),
                    &mut used_surface_ids,
                );
                surface_id_map.insert((*pane_id, surface.surface_id), surface_id.clone());
                workspace_surfaces.push(WorkspaceSurface {
                    id: surface_id,
                    title: surface.title.clone(),
                    owner: surface.owner.clone(),
                    cwd: layout_cwd(surface.cwd.as_deref().or(cwd.as_deref()), root),
                    close_pane_when_last: surface.close_pane_when_last,
                });
            }
            let active_surface = active_surface_id
                .and_then(|surface_id| surface_id_map.get(&(*pane_id, surface_id)).cloned())
                .or_else(|| workspace_surfaces.first().map(|surface| surface.id.clone()));

            panes.push(WorkspacePane {
                id: pane_id_text,
                title: title_hint.map(ToOwned::to_owned),
                cwd: layout_cwd(cwd.as_deref(), root),
                active_surface,
                surfaces: workspace_surfaces,
            });
        }
        PaneLayoutState::Split { first, second, .. } => {
            collect_workspace_panes(
                first,
                root,
                used_pane_ids,
                pane_id_map,
                surface_id_map,
                panes,
            );
            collect_workspace_panes(
                second,
                root,
                used_pane_ids,
                pane_id_map,
                surface_id_map,
                panes,
            );
        }
    }
}

fn workspace_node_from_session(
    node: &PaneLayoutState,
    pane_id_map: &HashMap<usize, String>,
) -> Option<WorkspaceLayoutNode> {
    match node {
        PaneLayoutState::Leaf { pane_id, .. } => pane_id_map
            .get(pane_id)
            .map(|id| WorkspaceLayoutNode::Pane { id: id.clone() }),
        PaneLayoutState::Split {
            direction,
            ratio,
            first,
            second,
        } => Some(WorkspaceLayoutNode::Split {
            direction: match direction {
                PaneSplitDirection::Horizontal => WorkspaceSplitDirection::Horizontal,
                PaneSplitDirection::Vertical => WorkspaceSplitDirection::Vertical,
            },
            ratio: ratio.clamp(0.05, 0.95),
            first: Box::new(workspace_node_from_session(first, pane_id_map)?),
            second: Box::new(workspace_node_from_session(second, pane_id_map)?),
        }),
    }
}

fn workspace_node_from_panes(panes: &[WorkspacePane]) -> Option<WorkspaceLayoutNode> {
    match panes {
        [] => None,
        [pane] => Some(WorkspaceLayoutNode::Pane {
            id: pane.id.clone(),
        }),
        _ => {
            let mid = panes.len().div_ceil(2);
            Some(WorkspaceLayoutNode::Split {
                direction: WorkspaceSplitDirection::Horizontal,
                ratio: (mid as f32 / panes.len() as f32).clamp(0.05, 0.95),
                first: Box::new(workspace_node_from_panes(&panes[..mid])?),
                second: Box::new(workspace_node_from_panes(&panes[mid..])?),
            })
        }
    }
}

fn session_node_from_workspace(
    node: &WorkspaceLayoutNode,
    tab: &WorkspaceTab,
    root: &Path,
    pane_id_map: &HashMap<&str, usize>,
) -> anyhow::Result<PaneLayoutState> {
    match node {
        WorkspaceLayoutNode::Pane { id } => {
            let pane_index = *pane_id_map
                .get(id.as_str())
                .ok_or_else(|| anyhow::anyhow!("layout references undefined pane {:?}", id))?;
            let pane = &tab.panes[pane_index];
            Ok(PaneLayoutState::Leaf {
                pane_id: pane_index,
                cwd: resolved_layout_cwd(pane.cwd.as_deref().or(tab.cwd.as_deref()), root),
                active_surface_id: active_surface_index(pane),
                surfaces: session_surfaces_from_workspace(pane, root),
            })
        }
        WorkspaceLayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => Ok(PaneLayoutState::Split {
            direction: match direction {
                WorkspaceSplitDirection::Horizontal => PaneSplitDirection::Horizontal,
                WorkspaceSplitDirection::Vertical => PaneSplitDirection::Vertical,
            },
            ratio: ratio.clamp(0.05, 0.95),
            first: Box::new(session_node_from_workspace(first, tab, root, pane_id_map)?),
            second: Box::new(session_node_from_workspace(second, tab, root, pane_id_map)?),
        }),
    }
}

fn session_surfaces_from_workspace(pane: &WorkspacePane, root: &Path) -> Vec<SurfaceState> {
    pane.surfaces
        .iter()
        .enumerate()
        .map(|(surface_id, surface)| SurfaceState {
            surface_id,
            title: surface.title.clone(),
            owner: surface.owner.clone(),
            cwd: resolved_layout_cwd(surface.cwd.as_deref().or(pane.cwd.as_deref()), root),
            close_pane_when_last: surface.close_pane_when_last,
            screen_text: Vec::new(),
        })
        .collect()
}

fn active_surface_index(pane: &WorkspacePane) -> Option<usize> {
    pane.active_surface
        .as_deref()
        .and_then(|id| pane.surfaces.iter().position(|surface| surface.id == id))
        .or_else(|| (!pane.surfaces.is_empty()).then_some(0))
}

fn workspace_tab_agent_from_routing(routing: &AgentRoutingState) -> WorkspaceTabAgent {
    let provider = routing
        .provider
        .as_ref()
        .map(|provider| provider.to_string());
    let model = routing
        .provider
        .as_ref()
        .and_then(|provider| {
            routing
                .model_overrides
                .iter()
                .find(|override_state| &override_state.provider == provider)
        })
        .or_else(|| routing.model_overrides.first())
        .map(|override_state| override_state.model.clone());

    WorkspaceTabAgent { provider, model }
}

fn agent_routing_from_workspace(
    provider: Option<ProviderKind>,
    model: Option<String>,
) -> AgentRoutingState {
    let Some(provider) = provider else {
        return AgentRoutingState::default();
    };
    let model_overrides = model
        .filter(|model| !model.trim().is_empty())
        .map(|model| {
            vec![AgentModelOverrideState {
                provider: provider.clone(),
                model,
            }]
        })
        .unwrap_or_default();

    AgentRoutingState {
        provider: Some(provider),
        model_overrides,
    }
}

fn parse_provider_kind(value: Option<&str>) -> Option<ProviderKind> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Some(ProviderKind::Anthropic),
        "openai" => Some(ProviderKind::OpenAI),
        "chatgpt" => Some(ProviderKind::ChatGPT),
        "github-copilot" | "githubcopilot" => Some(ProviderKind::GitHubCopilot),
        "openai-compatible" | "openai_compatible" | "openai compatible" => {
            Some(ProviderKind::OpenAICompatible)
        }
        "minimax" => Some(ProviderKind::MiniMax),
        "minimax-anthropic" => Some(ProviderKind::MiniMaxAnthropic),
        "moonshot" => Some(ProviderKind::Moonshot),
        "moonshot-anthropic" => Some(ProviderKind::MoonshotAnthropic),
        "z-ai" | "zai" => Some(ProviderKind::ZAI),
        "z-ai-anthropic" | "zai-anthropic" => Some(ProviderKind::ZAIAnthropic),
        "deepseek" => Some(ProviderKind::DeepSeek),
        "groq" => Some(ProviderKind::Groq),
        "cohere" => Some(ProviderKind::Cohere),
        "gemini" => Some(ProviderKind::Gemini),
        "ollama" => Some(ProviderKind::Ollama),
        "openrouter" => Some(ProviderKind::OpenRouter),
        "perplexity" => Some(ProviderKind::Perplexity),
        "mistral" => Some(ProviderKind::Mistral),
        "together" => Some(ProviderKind::Together),
        "xai" => Some(ProviderKind::XAI),
        _ => None,
    }
}

fn profile_input_history(history: Option<&GlobalHistoryState>) -> Vec<String> {
    history
        .map(|history| {
            if !history.input_history.is_empty() {
                history.input_history.clone()
            } else {
                history
                    .global_shell_history
                    .iter()
                    .map(|entry| entry.command.clone())
                    .filter(|command| !command.trim().is_empty())
                    .collect()
            }
        })
        .unwrap_or_default()
}

fn layout_cwd(cwd: Option<&str>, root: &Path) -> Option<String> {
    let cwd = cwd?.trim();
    if cwd.is_empty() {
        return None;
    }
    let path = Path::new(cwd);
    if path.is_absolute()
        && let Ok(relative) = path.strip_prefix(root)
    {
        return Some(layout_path_string(relative));
    }

    // Workspace layout files are meant to be reviewed and committed, so their
    // paths must be stable across platforms. Windows' `Path::strip_prefix` can
    // fail for Unix-looking test paths or mixed separators; keep a string-level
    // fallback so exports still produce repo-relative slash paths.
    let normalized_cwd = normalize_layout_path(cwd);
    let normalized_root = normalize_layout_path(&root.to_string_lossy());
    if let Some(relative) = strip_layout_root_prefix(&normalized_cwd, &normalized_root) {
        return Some(relative);
    }

    Some(normalized_cwd)
}

fn resolved_layout_cwd(cwd: Option<&str>, root: &Path) -> Option<String> {
    let cwd = cwd?.trim();
    if cwd.is_empty() {
        return None;
    }
    if cwd == "." {
        return Some(root.to_string_lossy().to_string());
    }

    let path = Path::new(cwd);
    if path.is_absolute() {
        Some(path.to_string_lossy().to_string())
    } else {
        Some(root.join(path).to_string_lossy().to_string())
    }
}

fn layout_path_string(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        return ".".to_string();
    }
    normalize_layout_path(&path.to_string_lossy())
}

fn normalize_layout_path(path: &str) -> String {
    path.trim().replace('\\', "/")
}

fn trim_layout_trailing_slashes(path: &str) -> &str {
    let mut end = path.len();
    while end > 1 && path[..end].ends_with('/') && !path[..end].ends_with(":/") {
        end -= 1;
    }
    &path[..end]
}

fn strip_layout_root_prefix(path: &str, root: &str) -> Option<String> {
    let path = trim_layout_trailing_slashes(path);
    let root = trim_layout_trailing_slashes(root);
    if root.is_empty() || root == "." {
        return None;
    }

    let same_path = if cfg!(windows) {
        path.eq_ignore_ascii_case(root)
    } else {
        path == root
    };
    if same_path {
        return Some(".".to_string());
    }

    let root_prefix = if root.ends_with('/') {
        root.to_string()
    } else {
        format!("{root}/")
    };
    let has_prefix = if cfg!(windows) {
        path.len() > root_prefix.len()
            && path
                .get(..root_prefix.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&root_prefix))
    } else {
        path.starts_with(&root_prefix)
    };
    if has_prefix {
        return path.get(root_prefix.len()..).map(ToOwned::to_owned);
    }

    None
}

fn slug_id(label: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn unique_id(base: &str, used: &mut HashSet<String>) -> String {
    let mut candidate = if base.trim().is_empty() {
        "item".to_string()
    } else {
        base.to_string()
    };
    if used.insert(candidate.clone()) {
        return candidate;
    }

    for suffix in 2usize.. {
        candidate = format!("{base}-{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("unbounded suffix search should always find a unique id")
}

fn validate_layout_node(
    node: &WorkspaceLayoutNode,
    pane_ids: &std::collections::HashSet<&str>,
    placed_panes: &mut std::collections::HashSet<String>,
    tab_id: &str,
) -> anyhow::Result<()> {
    match node {
        WorkspaceLayoutNode::Pane { id } => {
            anyhow::ensure!(
                pane_ids.contains(id.as_str()),
                "layout references undefined pane {:?} in tab {:?}",
                id,
                tab_id
            );
            anyhow::ensure!(
                placed_panes.insert(id.clone()),
                "layout references pane {:?} more than once in tab {:?}",
                id,
                tab_id
            );
        }
        WorkspaceLayoutNode::Split {
            ratio,
            first,
            second,
            ..
        } => {
            anyhow::ensure!(
                (0.05..=0.95).contains(ratio),
                "layout split ratio {} in tab {:?} is outside 0.05..=0.95",
                ratio,
                tab_id
            );
            validate_layout_node(first, pane_ids, placed_panes, tab_id)?;
            validate_layout_node(second, pane_ids, placed_panes, tab_id)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected_resolved_cwd(root: &str, relative: &str) -> String {
        Path::new(root).join(relative).to_string_lossy().to_string()
    }

    fn two_pane_layout() -> WorkspaceLayout {
        WorkspaceLayout {
            format: WORKSPACE_LAYOUT_FORMAT.to_string(),
            version: WORKSPACE_LAYOUT_VERSION,
            name: None,
            root: ".".to_string(),
            active_tab: Some("dev".to_string()),
            defaults: WorkspaceDefaults::default(),
            tabs: vec![WorkspaceTab {
                id: "dev".to_string(),
                title: None,
                cwd: Some(".".to_string()),
                active_pane: Some("a".to_string()),
                agent: WorkspaceTabAgent::default(),
                layout: Some(WorkspaceLayoutNode::Split {
                    direction: WorkspaceSplitDirection::Horizontal,
                    ratio: 0.5,
                    first: Box::new(WorkspaceLayoutNode::Pane {
                        id: "a".to_string(),
                    }),
                    second: Box::new(WorkspaceLayoutNode::Pane {
                        id: "b".to_string(),
                    }),
                }),
                panes: vec![
                    WorkspacePane {
                        id: "a".to_string(),
                        title: None,
                        cwd: Some(".".to_string()),
                        active_surface: Some("shell".to_string()),
                        surfaces: vec![WorkspaceSurface {
                            id: "shell".to_string(),
                            title: None,
                            owner: None,
                            cwd: Some(".".to_string()),
                            close_pane_when_last: false,
                        }],
                    },
                    WorkspacePane {
                        id: "b".to_string(),
                        title: None,
                        cwd: Some(".".to_string()),
                        active_surface: Some("shell".to_string()),
                        surfaces: vec![WorkspaceSurface {
                            id: "shell".to_string(),
                            title: None,
                            owner: None,
                            cwd: Some(".".to_string()),
                            close_pane_when_last: false,
                        }],
                    },
                ],
            }],
        }
    }

    #[test]
    fn workspace_layout_round_trips_as_ghostty_config() {
        let layout = WorkspaceLayout {
            format: WORKSPACE_LAYOUT_FORMAT.to_string(),
            version: WORKSPACE_LAYOUT_VERSION,
            name: Some("con".to_string()),
            root: ".".to_string(),
            active_tab: Some("dev".to_string()),
            defaults: WorkspaceDefaults {
                shell: Some("login".to_string()),
                agent_provider: Some("openai".to_string()),
                agent_model: Some("gpt-5.2".to_string()),
            },
            tabs: vec![WorkspaceTab {
                id: "dev".to_string(),
                title: Some("Dev".to_string()),
                cwd: Some(".".to_string()),
                active_pane: Some("editor".to_string()),
                agent: WorkspaceTabAgent::default(),
                layout: Some(WorkspaceLayoutNode::Split {
                    direction: WorkspaceSplitDirection::Horizontal,
                    ratio: 0.6,
                    first: Box::new(WorkspaceLayoutNode::Pane {
                        id: "editor".to_string(),
                    }),
                    second: Box::new(WorkspaceLayoutNode::Pane {
                        id: "server".to_string(),
                    }),
                }),
                panes: vec![
                    WorkspacePane {
                        id: "editor".to_string(),
                        title: Some("Editor".to_string()),
                        cwd: Some(".".to_string()),
                        active_surface: Some("shell".to_string()),
                        surfaces: vec![WorkspaceSurface {
                            id: "shell".to_string(),
                            title: Some("Shell".to_string()),
                            owner: None,
                            cwd: Some(".".to_string()),
                            close_pane_when_last: false,
                        }],
                    },
                    WorkspacePane {
                        id: "server".to_string(),
                        title: Some("Server".to_string()),
                        cwd: Some("crates/con-app".to_string()),
                        active_surface: Some("server".to_string()),
                        surfaces: vec![WorkspaceSurface {
                            id: "server".to_string(),
                            title: Some("Server".to_string()),
                            owner: None,
                            cwd: Some("crates/con-app".to_string()),
                            close_pane_when_last: false,
                        }],
                    },
                ],
            }],
        };

        let config = layout.to_ghostty_string().unwrap();
        assert!(config.contains("format = \"con.workspace.layout\""));
        assert!(config.contains("version = 2"));
        assert!(!config.contains("run ="));
        assert!(!config.contains("restore ="));

        let decoded = WorkspaceLayout::from_ghostty_str(&config).unwrap();
        assert_eq!(decoded, layout);
    }

    #[test]
    fn ghostty_roundtrip_preserves_asymmetric_tree_order_and_nontrivial_strings() {
        let mut layout = two_pane_layout();
        layout.name = Some("开发 = C:\\Users\\Zoë #1 \"quoted\"".into());
        layout.tabs[0].panes[0].surfaces.push(WorkspaceSurface {
            id: "日志".into(),
            title: Some("日志 = #1".into()),
            owner: Some("agent/windows".into()),
            cwd: Some(r#"C:\work dir\项目"#.into()),
            close_pane_when_last: true,
        });
        layout.tabs[0].panes.push(WorkspacePane {
            id: "c".into(),
            title: Some("Third".into()),
            cwd: None,
            active_surface: None,
            surfaces: vec![],
        });
        layout.tabs[0].layout = Some(WorkspaceLayoutNode::Split {
            direction: WorkspaceSplitDirection::Vertical,
            ratio: 0.3,
            first: Box::new(WorkspaceLayoutNode::Pane { id: "a".into() }),
            second: Box::new(WorkspaceLayoutNode::Split {
                direction: WorkspaceSplitDirection::Horizontal,
                ratio: 0.7,
                first: Box::new(WorkspaceLayoutNode::Pane { id: "b".into() }),
                second: Box::new(WorkspaceLayoutNode::Pane { id: "c".into() }),
            }),
        });
        let mut second = layout.tabs[0].clone();
        second.id = "second".into();
        second.title = Some("第二 tab".into());
        layout.tabs.push(second);

        let encoded = layout.to_ghostty_string().unwrap();
        let decoded = WorkspaceLayout::from_ghostty_str(&encoded).unwrap();
        assert_eq!(decoded, layout);
        assert!(encoded.find("pane = \"a\"").unwrap() < encoded.find("pane = \"b\"").unwrap());
    }

    #[test]
    fn workspace_layout_exports_git_stable_slash_paths() {
        assert_eq!(
            layout_cwd(
                Some("/tmp/project/crates/server"),
                Path::new("/tmp/project")
            )
            .as_deref(),
            Some("crates/server")
        );
        assert_eq!(
            layout_cwd(
                Some(r"C:\Users\me\project\crates\server"),
                Path::new(r"C:\Users\me\project")
            )
            .as_deref(),
            Some("crates/server")
        );
        assert_eq!(
            layout_cwd(Some(r"crates\server"), Path::new(r"C:\Users\me\project")).as_deref(),
            Some("crates/server")
        );
        assert_eq!(
            layout_cwd(Some("C:/project"), Path::new("C:/")).as_deref(),
            Some("project")
        );
    }

    #[test]
    fn workspace_layout_exports_native_absolute_paths_relative_to_root() {
        let root = std::env::current_dir().unwrap().join("project");
        let cwd = root.join("crates").join("server");
        let cwd = cwd.to_string_lossy();

        assert_eq!(
            layout_cwd(Some(&cwd), &root).as_deref(),
            Some("crates/server")
        );
    }

    #[test]
    fn workspace_layout_export_drops_private_runtime_state() {
        let session = Session {
            tabs: vec![TabState {
                title: "Dev".to_string(),
                cwd: Some("/tmp/project".to_string()),
                layout: Some(PaneLayoutState::Leaf {
                    pane_id: 7,
                    cwd: Some("/tmp/project".to_string()),
                    active_surface_id: Some(3),
                    surfaces: vec![SurfaceState {
                        surface_id: 3,
                        title: Some("Server".to_string()),
                        owner: Some("human".to_string()),
                        cwd: Some("/tmp/project/crates/server".to_string()),
                        close_pane_when_last: false,
                        screen_text: vec!["secret output".to_string()],
                    }],
                }),
                focused_pane_id: Some(7),
                panes: vec![PaneState {
                    cwd: Some("/tmp/project".to_string()),
                }],
                shell_history: vec![crate::session::PaneCommandHistoryState {
                    pane_id: Some(7),
                    entries: vec![CommandHistoryEntryState {
                        command: "cargo run".to_string(),
                        cwd: Some("/tmp/project".to_string()),
                    }],
                }],
                conversation_id: Some("private-conversation".to_string()),
                agent_routing: AgentRoutingState {
                    provider: Some(ProviderKind::OpenAI),
                    model_overrides: vec![AgentModelOverrideState {
                        provider: ProviderKind::OpenAI,
                        model: "gpt-5.2".to_string(),
                    }],
                },
                user_label: Some("Dev".to_string()),
                color: None,
            }],
            active_tab: 0,
            agent_panel_open: true,
            agent_panel_width: Some(420.0),
            input_bar_visible: false,
            global_shell_history: vec![CommandHistoryEntryState {
                command: "secret".to_string(),
                cwd: None,
            }],
            input_history: vec!["ask secret".to_string()],
            conversation_id: Some("legacy-private".to_string()),
            left_panel_width: Some(250.0),
            vertical_tabs_pinned: None,
            activity_slot: None,
            left_panel_open: None,
            editor_area_height: None,
        };

        let layout = WorkspaceLayout::from_session(&session, "/tmp/project");
        let toml = layout.to_ghostty_string().unwrap();

        assert!(toml.contains("active-tab = \"dev\""));
        assert!(toml.contains("surface.cwd = \"crates/server\""));
        assert!(toml.contains("tab.agent-provider = \"openai\""));
        assert!(toml.contains("tab.agent-model = \"gpt-5.2\""));
        assert!(!toml.contains("secret output"));
        assert!(!toml.contains("cargo run"));
        assert!(!toml.contains("private-conversation"));
        assert!(!toml.contains("screen_text"));
    }

    #[test]
    fn workspace_layout_import_creates_private_session_without_runtime_state() {
        let layout = WorkspaceLayout {
            format: WORKSPACE_LAYOUT_FORMAT.to_string(),
            version: WORKSPACE_LAYOUT_VERSION,
            name: Some("Project".to_string()),
            root: ".".to_string(),
            active_tab: Some("dev".to_string()),
            defaults: WorkspaceDefaults::default(),
            tabs: vec![WorkspaceTab {
                id: "dev".to_string(),
                title: Some("Dev".to_string()),
                cwd: Some(".".to_string()),
                active_pane: Some("server".to_string()),
                agent: WorkspaceTabAgent {
                    provider: Some("openai".to_string()),
                    model: Some("gpt-5.2".to_string()),
                },
                layout: Some(WorkspaceLayoutNode::Pane {
                    id: "server".to_string(),
                }),
                panes: vec![WorkspacePane {
                    id: "server".to_string(),
                    title: Some("Server".to_string()),
                    cwd: Some("crates/server".to_string()),
                    active_surface: Some("shell".to_string()),
                    surfaces: vec![WorkspaceSurface {
                        id: "shell".to_string(),
                        title: Some("Shell".to_string()),
                        owner: None,
                        cwd: Some("crates/server".to_string()),
                        close_pane_when_last: false,
                    }],
                }],
            }],
        };
        let history = GlobalHistoryState {
            global_shell_history: vec![CommandHistoryEntryState {
                command: "git status".to_string(),
                cwd: None,
            }],
            input_history: Vec::new(),
        };

        let session = layout.to_session("/tmp/project", Some(&history)).unwrap();

        assert_eq!(session.active_tab, 0);
        assert_eq!(session.tabs[0].cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(session.tabs[0].focused_pane_id, Some(0));
        assert!(session.tabs[0].conversation_id.is_none());
        assert_eq!(session.tabs[0].shell_history[0].entries.len(), 0);
        assert_eq!(session.global_shell_history[0].command, "git status");
        assert_eq!(
            session.tabs[0].agent_routing.provider,
            Some(ProviderKind::OpenAI)
        );
        match session.tabs[0].layout.as_ref().unwrap() {
            PaneLayoutState::Leaf { surfaces, .. } => {
                assert_eq!(
                    surfaces[0].cwd.as_deref(),
                    Some(expected_resolved_cwd("/tmp/project", "crates/server").as_str())
                );
                assert!(surfaces[0].screen_text.is_empty());
            }
            PaneLayoutState::Split { .. } => panic!("expected leaf"),
        }
    }

    #[test]
    fn workspace_layout_rejects_dangling_layout_panes() {
        let toml = r#"
format = "con.workspace.layout"
version = 1

[[tabs]]
id = "dev"
layout = { kind = "pane", id = "missing" }
panes = []
"#;

        let err = WorkspaceLayout::from_toml_str(toml).unwrap_err();
        assert!(err.to_string().contains("undefined pane"));
    }

    #[test]
    fn workspace_layout_rejects_duplicate_layout_pane_refs() {
        let mut layout = two_pane_layout();
        layout.tabs[0].layout = Some(WorkspaceLayoutNode::Split {
            direction: WorkspaceSplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(WorkspaceLayoutNode::Pane {
                id: "a".to_string(),
            }),
            second: Box::new(WorkspaceLayoutNode::Pane {
                id: "a".to_string(),
            }),
        });

        let err = layout.validate().unwrap_err();
        assert!(err.to_string().contains("more than once"));
    }

    #[test]
    fn workspace_layout_rejects_layout_that_omits_panes() {
        let mut layout = two_pane_layout();
        layout.tabs[0].layout = Some(WorkspaceLayoutNode::Pane {
            id: "a".to_string(),
        });

        let err = layout.validate().unwrap_err();
        assert!(err.to_string().contains("exactly once"));
    }

    #[test]
    fn workspace_layout_rejects_multi_pane_tabs_without_layout() {
        let mut layout = two_pane_layout();
        layout.tabs[0].layout = None;

        let err = layout.validate().unwrap_err();
        assert!(err.to_string().contains("must define layout"));
    }

    #[test]
    fn workspace_layout_export_synthesizes_layout_for_legacy_multi_pane_tabs() {
        let session = Session {
            tabs: vec![TabState {
                title: "Legacy".to_string(),
                cwd: Some("/tmp/project".to_string()),
                layout: None,
                focused_pane_id: Some(1),
                panes: vec![
                    PaneState {
                        cwd: Some("/tmp/project".to_string()),
                    },
                    PaneState {
                        cwd: Some("/tmp/project/crates/server".to_string()),
                    },
                ],
                shell_history: Vec::new(),
                conversation_id: None,
                agent_routing: AgentRoutingState::default(),
                user_label: None,
                color: None,
            }],
            active_tab: 0,
            agent_panel_open: false,
            agent_panel_width: None,
            input_bar_visible: true,
            global_shell_history: Vec::new(),
            input_history: Vec::new(),
            conversation_id: None,
            left_panel_width: None,
            vertical_tabs_pinned: None,
            activity_slot: None,
            left_panel_open: None,
            editor_area_height: None,
        };

        let layout = WorkspaceLayout::from_session(&session, "/tmp/project");

        layout.validate().unwrap();
        assert!(matches!(
            layout.tabs[0].layout,
            Some(WorkspaceLayoutNode::Split { .. })
        ));
        assert_eq!(layout.tabs[0].panes.len(), 2);
    }

    #[test]
    fn workspace_layout_rejects_unknown_format() {
        let toml = r#"
format = "other.layout"
version = 1
"#;

        let err = WorkspaceLayout::from_toml_str(toml).unwrap_err();
        assert!(
            err.to_string()
                .contains("unsupported workspace layout format")
        );
    }

    #[test]
    fn legacy_toml_import_is_upgraded_without_writing() {
        let legacy = r#"format = "con.workspace.layout"
version = 1
name = "旧 project"
root = "."
"#;
        let layout = WorkspaceLayout::from_toml_str(legacy).unwrap();
        assert_eq!(layout.version, WORKSPACE_LAYOUT_VERSION);
        assert_eq!(layout.name.as_deref(), Some("旧 project"));
    }

    #[test]
    fn native_profile_wins_and_legacy_profile_is_read_only() {
        let root = std::env::temp_dir().join(format!("con-layout-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".con")).unwrap();
        let legacy = root.join(".con/workspace.toml");
        let original = "format = \"con.workspace.layout\"\nversion = 1\nroot = \".\"\n";
        std::fs::write(&legacy, original).unwrap();
        assert_eq!(WorkspaceLayout::existing_path_for_root(&root), legacy);
        let layout = WorkspaceLayout::load(&legacy).unwrap();
        assert!(layout.save(&legacy).is_err());
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), original);
        let native = WorkspaceLayout::default_path_for_root(&root);
        layout.save(&native).unwrap();
        assert_eq!(WorkspaceLayout::load(&native).unwrap(), layout);
        std::fs::write(&native, "invalid new profile").unwrap();
        assert_eq!(WorkspaceLayout::existing_path_for_root(&root), native);
        assert!(WorkspaceLayout::load(&native).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
