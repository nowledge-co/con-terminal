use super::*;
use con_agent::chatgpt_subscription::{
    Catalog, DEFAULT_MODEL, Model, fallback_models, retired_model_replacement,
};

fn model_capabilities(config: &ProviderConfig) -> Option<Model> {
    let id = config
        .model
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_MODEL);
    if let Some(catalog) = Catalog::load(config) {
        return catalog.models.into_iter().find(|model| model.id == id);
    }
    fallback_models()
        .iter()
        .find(|model| model.id == id)
        .cloned()
}

fn reasoning_options(config: &ProviderConfig) -> Vec<String> {
    let mut options = vec!["Provider default".to_owned()];
    if let Some(model) = model_capabilities(config) {
        options.extend(
            model
                .reasoning_efforts
                .iter()
                .map(|effort| effort.as_str().to_owned()),
        );
    }
    // Keep authored values visible until the user explicitly changes them.
    if let Some(effort) = config.reasoning_effort {
        if !options.iter().any(|v| v == effort.as_str()) {
            options.push(effort.as_str().into());
        }
    }
    options
}

impl SettingsPanel {
    pub(super) fn make_reasoning_select(
        config: &ProviderConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SelectState<Vec<String>>> {
        let values = reasoning_options(config);
        let selected = config
            .reasoning_effort
            .map(|effort| effort.as_str())
            .unwrap_or("Provider default");
        let index = values
            .iter()
            .position(|v| v == selected)
            .map(IndexPath::new);
        let entity = cx.new(|cx| SelectState::new(values, index, window, cx));
        cx.subscribe(&entity, |_, _, _: &SelectEvent<Vec<String>>, cx| {
            cx.notify()
        })
        .detach();
        entity
    }

    pub(super) fn load_reasoning_options(
        &self,
        config: &ProviderConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reasoning_select.update(cx, |state, cx| {
            state.set_items(reasoning_options(config), window, cx);
            let value = config
                .reasoning_effort
                .map(|effort| effort.as_str())
                .unwrap_or("Provider default")
                .to_owned();
            state.set_selected_value(&value, window, cx);
        });
    }

    pub(super) fn subscription_controls(&self, cx: &Context<Self>) -> Div {
        let config = self.read_provider_inputs(cx);
        let id = config
            .model
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_MODEL);
        let today = chrono::Utc::now().date_naive();
        let warning = if let Some(replacement) = retired_model_replacement(id, today) {
            Some(format!(
                "{id} has retired from ChatGPT subscriptions. Select {replacement}."
            ))
        } else if id == "gpt-5.5" {
            Some("GPT-5.5 retires on October 14. Select GPT-5.6 Sol before then.".into())
        } else if config.reasoning_effort.is_some_and(|effort| {
            model_capabilities(&config).is_some_and(|m| !m.reasoning_efforts.contains(&effort))
        }) {
            Some("The saved reasoning effort is not supported by this model. Choose Provider default or another effort.".into())
        } else {
            None
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().text_sm().child("Reasoning effort"))
            .child(Select::new(&self.reasoning_select).small())
            .children(
                warning.map(|message| div().text_xs().text_color(cx.theme().danger).child(message)),
            )
    }
}
