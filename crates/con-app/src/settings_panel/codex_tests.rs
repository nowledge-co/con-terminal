use super::{ProviderKind, SettingsPanel, SettingsSection};

#[gpui::test]
fn local_codex_settings_render_without_http_credentials_in_both_themes(
    cx: &mut gpui::TestAppContext,
) {
    con_core::release_channel::init();
    cx.update(gpui_component::init);
    for mode in [
        gpui_component::ThemeMode::Light,
        gpui_component::ThemeMode::Dark,
    ] {
        let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().unwrap());
        let (panel, view) = cx.add_window_view(move |window, cx| {
            gpui_component::Theme::change(mode, Some(window), cx);
            let mut config = con_core::Config::default();
            config.agent.select_provider(ProviderKind::CodexLocal);
            let mut panel = SettingsPanel::new(
                &config,
                crate::model_registry::ModelRegistry::new(),
                runtime,
                window,
                cx,
            );
            panel.standalone = true;
            panel.visible = true;
            panel.active_section = SettingsSection::Providers;
            panel
        });
        for (width, height) in [(920.0, 680.0), (600.0, 600.0)] {
            view.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            assert!(view.debug_bounds("settings-provider-Codex Local").is_some());
            let fetch = view
                .debug_bounds("settings-provider-fetch")
                .expect("local model fetch button");
            assert!(fetch.size.width > gpui::px(0.0));
            assert!(
                fetch.right() <= gpui::px(width),
                "Codex fetch control overflowed: {fetch:?}"
            );
            for selector in [
                "settings-provider-field-API Key",
                "settings-provider-field-Base URL",
                "settings-provider-field-Max Tokens",
            ] {
                assert!(
                    view.debug_bounds(selector).is_none(),
                    "ignored HTTP field {selector} was shown for Codex Local"
                );
            }
        }
        panel.update(view, |panel, cx| {
            assert_eq!(panel.selected_provider, ProviderKind::CodexLocal);
            assert!(panel.read_provider_inputs(cx).model.is_none());
            panel
                .registry
                .set_codex_local_models(vec!["local-fixture-model".into()]);
            cx.notify();
        });
        // A fetched catalog must not silently replace the default Codex model.
        assert!(view.debug_bounds("settings-provider-fetch").is_some());
        panel.update(view, |panel, cx| {
            assert!(panel.read_provider_inputs(cx).model.is_none())
        });
        view.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.select_provider(ProviderKind::OpenAI, window, cx)
            });
        });
        // Changing away from Local must restore existing HTTP provider controls.
        assert!(
            view.debug_bounds("settings-provider-field-API Key")
                .is_some()
        );
        assert!(
            view.debug_bounds("settings-provider-field-Base URL")
                .is_some()
        );
    }
}
