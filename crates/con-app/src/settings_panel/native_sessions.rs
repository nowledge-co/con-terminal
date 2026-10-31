//! Recovery is automatic; only failures need an explicit retry action.
use super::*;

#[derive(Default)]
pub(super) struct NativeSessionSetup {
    running: bool,
    result: Option<Result<String, String>>,
}

impl SettingsPanel {
    fn retry_automatic_integration(&mut self, cx: &mut Context<Self>) {
        if self.native_session_setup.running || !self.config.appearance.resume_agent_sessions {
            return;
        }
        self.native_session_setup.running = true;
        self.native_session_setup.result = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async {
                    crate::native_agent::prepare_automatic_integration(true).map(|_| {
                        "Ready. Open a new con terminal to use session recovery.".to_string()
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.native_session_setup.running = false;
                this.native_session_setup.result = Some(result);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_native_session_setup(&self, mobile: bool, cx: &Context<Self>) -> Div {
        let error = crate::native_agent::automatic_integration_error();
        let state = &self.native_session_setup;
        let message = state
            .result
            .as_ref()
            .map(|result| match result {
                Ok(message) => (message.clone(), false),
                Err(message) => (message.clone(), true),
            })
            .or_else(|| {
                error.map(|error| (format!("Session recovery needs attention. {error}"), true))
            });
        let Some((message, failed)) = message else {
            return div();
        };
        div()
            .flex()
            .when(mobile, |view| view.flex_col())
            .items_start()
            .justify_between()
            .gap(px(12.0))
            .px(rems(1.0))
            .pb(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .line_height(rems(1.125))
                    .text_color(if failed {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(message),
            )
            .when(failed, |view| {
                view.child(
                    Button::new("retry-native-integration")
                        .label(if state.running {
                            "Preparing…"
                        } else {
                            "Retry"
                        })
                        .small()
                        .disabled(state.running || !self.config.appearance.resume_agent_sessions)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.retry_automatic_integration(cx)),
                        ),
                )
            })
    }
}
