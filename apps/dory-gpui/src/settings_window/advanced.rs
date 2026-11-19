//! Settings > Advanced > Speech vocabulary (UX-19, UX-32): the words and names the speech model
//! listens harder for, each Normal or Strong, and the names Dory adds from renamed speakers with
//! a Hide button on each. The core owns every rule (`dory_core::domain::vocabulary`); this
//! module words them and shows a rejected term's message under the field.

use dory_core::domain::{Boost, MAX_SPEAKER_NAMES, Settings, VocabularyTerm, folded_words};
use dory_core::models::Stt;
use gpui_kit::component::input::Input;
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, prelude::FluentBuilder as _, px,
    relative,
};

use super::{SettingsWindow, row, row_with, section};
use crate::a11y;
use crate::icons::{self, Icon};
use crate::models_view::{self, button, note};
use crate::radio::{self, Radio};
use crate::segmented::{self, Choice};
use crate::theme::Theme;

pub const ABOUT: [&str; 3] = [
    "Dory listens harder for these words when it transcribes. Add names and terms it gets wrong, such as a colleague's name or a product.",
    "Normal favors a word gently. Strong pushes much harder, for a word Normal does not fix, but it can turn words that sound alike into it. Use Strong only when you need it.",
    "Changes apply to recordings transcribed from now on. Re-transcribe a recording to use them there. Transcribing takes a little longer while the list has words in it.",
];
pub const EMPTY: &str = "No words yet.";
pub const SPEAKERS_LABEL: &str = "Add names you give speakers";
pub const NO_NAMES: &str = "No speaker names yet. Rename a speaker in a recording to add one.";
pub const NAMES_OFF: &str = "Names are not added while this is off.";
const NORMAL_EFFECT: &str = "Favors this word gently, without changing words that sound alike.";
const STRONG_EFFECT: &str =
    "Pushes the speech model harder toward this word. It can turn words that sound alike into it.";

/// A word's boosts in the order its switch shows them.
const BOOSTS: [Boost; 2] = [Boost::Normal, Boost::Strong];

/// The Normal and Strong choices on `text`'s switch, with what each does for VoiceOver.
fn boost_choices(text: &str) -> [Choice; 2] {
    [
        Choice {
            id: format!("vocabulary-normal-{text}").into(),
            label: "Normal".into(),
            description: Some(NORMAL_EFFECT.into()),
        },
        Choice {
            id: format!("vocabulary-strong-{text}").into(),
            label: "Strong".into(),
            description: Some(STRONG_EFFECT.into()),
        },
    ]
}

/// What sits under the field: a rejected term's message, or the word that is already listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    Rejected(SharedString),
    Listed(SharedString),
}

impl Said {
    fn text(&self) -> &SharedString {
        match self {
            Said::Rejected(text) | Said::Listed(text) => text,
        }
    }
}

/// The speaker names to offer for hiding: the core's added names, less any the user hid a moment
/// ago and any that are now a term of their own.
pub fn offered_names(added: &[String], settings: &Settings) -> Vec<String> {
    let hidden: Vec<Vec<String>> = settings
        .vocabulary_hidden_names
        .iter()
        .map(|n| folded_words(n))
        .collect();
    let terms: Vec<Vec<String>> = settings
        .vocabulary
        .iter()
        .map(|t| folded_words(&t.text))
        .collect();
    added
        .iter()
        .filter(|name| {
            let words = folded_words(name);
            !hidden.contains(&words) && !terms.contains(&words)
        })
        .cloned()
        .collect()
}

impl SettingsWindow {
    /// Reads the names the core adds from speakers, off the main thread, when the pane opens and
    /// after the settings are saved and read back.
    pub(super) fn read_speaker_names(&mut self, cx: &mut Context<Self>) {
        self.names_read_at = self.models.read(cx).refreshes();
        self.names_generation += 1;
        let generation = self.names_generation;
        let read = self.app.read(cx).read_speech_vocabulary(cx);
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| {
                if this.names_generation != generation {
                    return;
                }
                let kept = this
                    .models
                    .read(cx)
                    .settings()
                    .map_or(0, |s| s.vocabulary.len());
                this.speaker_names = Some(read.map(|terms| {
                    // The user's terms come first, then the added names.
                    terms.into_iter().skip(kept).map(|t| t.text).collect()
                }));
                cx.notify();
            });
        })
        .detach();
    }

    /// Adds the field's text as a Normal term. A rejected term keeps its text and shows why.
    pub(super) fn add_term(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.vocab_field.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let words = folded_words(&text);
        let listed = self.models.read(cx).settings().and_then(|settings| {
            settings
                .vocabulary
                .iter()
                .find(|t| !words.is_empty() && folded_words(&t.text) == words)
                .map(|t| t.text.clone())
        });
        if let Some(listed) = listed {
            let said = format!("{listed} is already in the list.");
            a11y::announce(&said);
            self.vocab_said = Some(Said::Listed(said.into()));
            cx.notify();
            return;
        }
        let add = text.clone();
        let saved = self.app.update(cx, |app, cx| {
            app.edit_settings(
                move |s| s.vocabulary.push(VocabularyTerm::new(add, Boost::Normal)),
                cx,
            )
        });
        cx.spawn_in(window, async move |this, cx| {
            let saved = saved.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match saved {
                    Ok(()) => {
                        a11y::announce(&format!("Added {text}"));
                        this.vocab_said = None;
                        this.vocab_field
                            .update(cx, |field, cx| field.set_value("", window, cx));
                    }
                    Err(error) => {
                        a11y::announce(&error);
                        this.vocab_said = Some(Said::Rejected(error.into()));
                    }
                }
                this.models.update(cx, |m, cx| m.refresh(cx));
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn advanced(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(settings) = self.models.read(cx).settings().cloned() else {
            return vec![section(
                "settings-vocabulary",
                "Speech vocabulary",
                theme,
                vec![row("setting-vocabulary", "Loading", "", None, theme).into_any_element()],
            )];
        };
        let about = self.about(&settings, theme, cx);
        let add = self.add_row(theme, cx);
        let words = self.words(&settings, theme, cx);
        let speakers = self.speakers(&settings, theme, window, cx);
        vec![
            section(
                "settings-vocabulary",
                "Speech vocabulary",
                theme,
                vec![
                    self.mark("row-vocabulary", about, theme, cx),
                    self.mark("row-vocabulary-add", add, theme, cx),
                    self.mark("row-vocabulary-words", words, theme, cx),
                ],
            ),
            section(
                "settings-vocabulary-speakers",
                "Names from speakers",
                theme,
                speakers,
            ),
        ]
    }

    /// The three plain sentences, and under them a line when the resolved speech model does not
    /// use the list.
    fn about(&self, settings: &Settings, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let lines = ABOUT.iter().enumerate().map(|(ix, line)| {
            a11y::node(format!("vocabulary-about-{ix}"), Role::Label, *line)
                .text_size(theme.text(if ix == 0 { 13. } else { 12. }))
                .line_height(relative(1.4))
                .when(ix > 0, |l| l.text_color(theme.text_secondary))
                .child(*line)
        });
        let unused = self.models.read(cx).rows::<Stt>().and_then(|rows| {
            let entry = models_view::resolved_speech(rows, settings)?;
            if entry.caps.vocabulary {
                return None;
            }
            let names = rows
                .iter()
                .map(|r| &r.status.entry)
                .find(|e| e.caps.vocabulary)
                .map(|e| e.display_name.clone());
            Some(match names {
                Some(other) => format!(
                    "Your speech model, {}, does not use this list. {other} does.",
                    entry.display_name
                ),
                None => format!(
                    "Your speech model, {}, does not use this list. Pick a speech model that supports it in Transcription.",
                    entry.display_name
                ),
            })
        });
        div_col()
            .px(px(14.))
            .py(px(11.))
            .gap(px(6.))
            .children(lines)
            .children(unused.map(|text| {
                a11y::node("vocabulary-unused", Role::Status, text.clone())
                    .text_size(theme.text(12.))
                    .line_height(relative(1.4))
                    .child(icons::beside(
                        Icon::Info,
                        12.,
                        theme.text_secondary,
                        theme.text(12.) * 1.4,
                        text,
                    ))
            }))
            .into_any_element()
    }

    /// The field with Return, the Add button, and what was said about the last term.
    fn add_row(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let focus = self.focus("vocabulary-add", cx);
        let this = cx.entity().downgrade();
        let add = button(
            "vocabulary-add",
            "Add word",
            "Add",
            true,
            &focus,
            theme,
            move |window, cx| {
                let _ = this.update(cx, |this, cx| this.add_term(window, cx));
            },
        );
        let said = self.vocab_said.clone().map(|said| {
            let text = said.text().clone();
            let rejected = matches!(said, Said::Rejected(_));
            let color = if rejected {
                theme.attention
            } else {
                theme.text_secondary
            };
            a11y::node("vocabulary-said", Role::Status, text.clone())
                .text_size(theme.text(12.))
                .text_color(color)
                .child(icons::beside(
                    if rejected {
                        Icon::TriangleAlert
                    } else {
                        Icon::Info
                    },
                    12.,
                    color,
                    theme.text(12.) * 1.4,
                    text,
                ))
        });
        let below = div_col()
            .gap(px(6.))
            .pt(px(6.))
            .child(
                gpui_kit::div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        gpui_kit::div().flex_1().min_w_0().child(
                            Input::new(&self.vocab_field)
                                .id("vocabulary-field")
                                .aria_label("Add a word or name"),
                        ),
                    )
                    .child(add),
            )
            .children(said)
            .into_any_element();
        row_with(
            "setting-vocabulary-add",
            "Add a word or name",
            below,
            None,
            theme,
        )
        .into_any_element()
    }

    /// Each word with its Normal / Strong switch and an X to remove it, or the empty line.
    fn words(&mut self, settings: &Settings, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if settings.vocabulary.is_empty() {
            return a11y::node("vocabulary-empty", Role::Label, EMPTY)
                .px(px(14.))
                .py(px(11.))
                .child(note(EMPTY, theme).text_size(theme.text(13.)))
                .into_any_element();
        }
        let rows: Vec<AnyElement> = settings
            .vocabulary
            .iter()
            .map(|term| self.word(term, theme, cx))
            .collect();
        a11y::node("vocabulary-words", Role::List, "Words")
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    }

    fn word(&mut self, term: &VocabularyTerm, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let text = term.text.clone();
        let focus = [
            self.focus(format!("vocabulary-normal-{text}"), cx),
            self.focus(format!("vocabulary-strong-{text}"), cx),
        ];
        let models = self.models.clone();
        let target = text.clone();
        let boost = segmented::switch(
            (
                format!("vocabulary-boost-{text}"),
                format!("Boost for {text}"),
            ),
            boost_choices(&text),
            &focus,
            BOOSTS.iter().position(|b| *b == term.boost).unwrap_or(0),
            move |ix, cx| {
                let target = target.clone();
                let next = BOOSTS[ix];
                models.update(cx, |m, cx| {
                    m.edit(
                        move |s| {
                            if let Some(t) = s.vocabulary.iter_mut().find(|t| t.text == target) {
                                t.boost = next;
                            }
                        },
                        cx,
                    )
                })
            },
            theme,
        );
        let remove_focus = self.focus(format!("vocabulary-remove-{text}"), cx);
        let models = self.models.clone();
        let target = text.clone();
        let remove = a11y::focusable(
            a11y::node(
                format!("vocabulary-remove-{text}"),
                Role::Button,
                format!("Remove {text}"),
            ),
            &remove_focus,
            theme,
            false,
        )
        .on_click(move |_, _, cx| {
            a11y::announce(&format!("Removed {target}"));
            let target = target.clone();
            models.update(cx, |m, cx| {
                m.edit(move |s| s.vocabulary.retain(|t| t.text != target), cx)
            });
        })
        .cursor_pointer()
        .flex_none()
        .size(icons::scaled(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .child(Icon::X.view(14., theme.text_secondary));
        a11y::node(
            format!("vocabulary-term-{text}"),
            Role::ListItem,
            text.clone(),
        )
        .px(px(14.))
        .py(px(6.))
        .min_h(px(40.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            gpui_kit::div()
                .flex_1()
                .min_w_0()
                .text_size(theme.text(13.))
                .child(text),
        )
        .child(boost)
        .child(remove)
        .into_any_element()
    }

    /// The switch, then the names added from speakers with Hide, and the hidden ones with Show.
    fn speakers(
        &mut self,
        settings: &Settings,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let on = settings.vocabulary_from_speakers;
        let focus = self.focus("vocabulary-speakers", cx);
        let models = self.models.clone();
        let switch = radio::checkbox(
            Radio::new("setting-vocabulary-speakers", SPEAKERS_LABEL).description(format!(
                "When you rename a speaker in any recording, Dory adds that name here, up to the {MAX_SPEAKER_NAMES} newest."
            )),
            on,
            &focus,
            theme,
            window,
            move |_, cx| models.update(cx, |m, cx| m.edit(move |s| s.vocabulary_from_speakers = !on, cx)),
        );
        let mut rows = vec![self.mark("row-vocabulary-speakers", switch, theme, cx)];
        let names = if on {
            self.names(settings, theme, cx)
        } else {
            plain_line("vocabulary-names-off", NAMES_OFF, theme)
        };
        rows.push(self.mark("row-vocabulary-names", names, theme, cx));
        rows
    }

    fn names(&mut self, settings: &Settings, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let offered = match &self.speaker_names {
            None => return plain_line("vocabulary-names-loading", "Reading names", theme),
            Some(Err(error)) => {
                return plain_line("vocabulary-names-failed", error.clone(), theme);
            }
            Some(Ok(added)) => offered_names(added, settings),
        };
        let mut body = div_col().py(px(4.));
        if offered.is_empty() {
            body = body.child(plain_line("vocabulary-names-none", NO_NAMES, theme));
        }
        for name in offered {
            let focus = self.focus(format!("vocabulary-hide-{name}"), cx);
            let models = self.models.clone();
            let target = name.clone();
            let hide = button(
                format!("vocabulary-hide-{name}"),
                format!("Hide {name}"),
                "Hide",
                false,
                &focus,
                theme,
                move |_, cx| {
                    a11y::announce(&format!("Hid {target}"));
                    let target = target.clone();
                    models.update(cx, |m, cx| {
                        m.edit(
                            move |s| {
                                let words = folded_words(&target);
                                if !s
                                    .vocabulary_hidden_names
                                    .iter()
                                    .any(|h| folded_words(h) == words)
                                {
                                    s.vocabulary_hidden_names.push(target.clone());
                                }
                            },
                            cx,
                        )
                    });
                },
            );
            body = body.child(name_row(
                format!("vocabulary-name-{name}"),
                &name,
                hide,
                false,
                theme,
            ));
        }
        for name in &settings.vocabulary_hidden_names {
            let focus = self.focus(format!("vocabulary-show-{name}"), cx);
            let models = self.models.clone();
            let target = name.clone();
            let show = button(
                format!("vocabulary-show-{name}"),
                format!("Show {name}"),
                "Show",
                false,
                &focus,
                theme,
                move |_, cx| {
                    a11y::announce(&format!("Showing {target}"));
                    let target = target.clone();
                    models.update(cx, |m, cx| {
                        m.edit(
                            move |s| s.vocabulary_hidden_names.retain(|h| *h != target),
                            cx,
                        )
                    });
                },
            );
            body = body.child(name_row(
                format!("vocabulary-hidden-{name}"),
                name,
                show,
                true,
                theme,
            ));
        }
        body.into_any_element()
    }
}

fn div_col() -> gpui_kit::Div {
    gpui_kit::div().flex().flex_col()
}

fn plain_line(id: &'static str, text: impl Into<SharedString>, theme: &Theme) -> AnyElement {
    let text: SharedString = text.into();
    a11y::node(id, Role::Label, text.clone())
        .px(px(14.))
        .py(px(7.))
        .child(note(text, theme))
        .into_any_element()
}

/// One name with its Hide or Show button. A hidden name reads struck from the list: dimmed, with
/// "Hidden" after it, so the state is words as well as color.
fn name_row(id: String, name: &str, action: a11y::Node, hidden: bool, theme: &Theme) -> AnyElement {
    let label = if hidden {
        format!("{name}, hidden")
    } else {
        name.to_owned()
    };
    a11y::node(id, Role::ListItem, label)
        .px(px(14.))
        .py(px(4.))
        .min_h(px(36.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            gpui_kit::div()
                .flex_1()
                .min_w_0()
                .text_size(theme.text(13.))
                .when(hidden, |d| d.text_color(theme.text_secondary))
                .child(name.to_owned())
                .when(hidden, |d| d.child("  Hidden")),
        )
        .child(action)
        .into_any_element()
}
