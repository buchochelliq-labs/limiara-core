//! How the tool talks to the user.
//!
//! Questions (a choice, a line of text, the step overview) are small
//! intuiTUIve apps run inline, in a few rows under the prompt; output that
//! streams is plain rich console output. A yes/no question is rich's
//! `Confirm`. Without a terminal (piped input) every question falls back
//! to rich's line prompts.
//!
//! No question here ever asks for a password or a code: those are typed
//! into `xtool` or `sudo` directly, with no TUI active.

use std::io::IsTerminal;
use std::rc::Rc;

use intuituive::prelude::*;
use intuituive::{radio, steps, StepState};
use rich::markup::escape;
use rich::panel::Panel;
use rich::prompt::{Confirm, Prompt};
use rich::rule::Rule;
use rich::Console;

use crate::cmd::Cmd;
use crate::error::{Error, Result};

pub type Validator = Rc<dyn Fn(&str) -> std::result::Result<String, String>>;

/// One line of the step overview.
#[derive(Clone, Debug)]
pub struct OverviewItem {
    pub title: String,
    pub state: StepState,
    pub detail: String,
}

pub trait Ui {
    fn console(&self) -> &Console;

    /// Pick one of `options` (console markup is escaped). Returns its index.
    fn choose(&mut self, question: &str, options: &[String], default: usize) -> Result<usize>;

    /// A line of text, re-asked until `validate` accepts it; returns what
    /// `validate` made of it.
    fn ask(&mut self, question: &str, default: Option<&str>, validate: Validator)
        -> Result<String>;

    /// A yes/no question about something that is not a secret.
    fn confirm(&mut self, question: &str, default: bool) -> Result<bool>;

    /// Show the steps and wait for the go-ahead.
    fn overview(&mut self, items: &[OverviewItem]) -> Result<bool>;

    // Output.

    fn say(&self, markup: &str) {
        self.console().print_str(markup);
    }

    /// Text printed exactly as given (no markup).
    fn say_verbatim(&self, text: &str) {
        self.console().print_str(&escape(text));
    }

    fn heading(&self, title: &str) {
        self.console().print_str("");
        self.console()
            .print(&Rule::new(format!("[bold]{}[/]", escape(title))));
    }

    /// A short explanation in a panel: what is about to happen and why.
    fn explain(&self, title: &str, body_markup: &str) {
        let text = self.console().build_text(body_markup);
        self.console().print(
            &Panel::new(Box::new(text))
                .title(escape(title))
                .border_style("blue"),
        );
    }

    fn warn(&self, title: &str, body_markup: &str) {
        let text = self.console().build_text(body_markup);
        self.console().print(
            &Panel::new(Box::new(text))
                .title(format!("[bold yellow]{}[/]", escape(title)))
                .border_style("yellow"),
        );
    }

    fn done(&self, markup: &str) {
        self.say(&format!("[green]✓[/] {markup}"));
    }

    fn note(&self, markup: &str) {
        self.say(&format!("[cyan]›[/] {markup}"));
    }

    fn show_command(&self, cmd: &Cmd) {
        self.say(&format!("  [bold]$ {}[/]", escape(&cmd.display())));
    }
}

/// The console the tool prints with: no emoji codes, so text from xtool is
/// shown exactly.
pub fn make_console() -> Console {
    Console::builder().emoji(false).build()
}

/// The choice screen: the question, the options as radio buttons, Enter
/// to choose and Esc to cancel.
pub fn choice_app(question: &str, options: &[String], default: usize) -> App {
    let question = question.to_string();
    let options: Vec<String> = options.iter().map(|o| escape(o)).collect();
    let default = default.min(options.len().saturating_sub(1));
    App::new(move || {
        let selected = signal(default);
        let opts = options.clone();
        column([
            label(format!("[bold]? {}[/]", escape(&question))),
            radio(move || opts.clone(), selected).label(question.clone()),
            label("[dim]↑/↓ to move · Enter to choose · Esc to cancel[/]"),
        ])
        .on_key("enter", move |cx| cx.quit_with(selected.get()))
        .on_key("esc", |cx| cx.quit())
    })
}

pub fn choice_rows(options: &[String]) -> u16 {
    u16::try_from(options.len() + 2).unwrap_or(u16::MAX)
}

/// The text question: a single-line input, with the validation message
/// under it while what was typed is not accepted.
pub fn input_app(question: &str, default: Option<&str>, validate: Validator) -> App {
    let question = question.to_string();
    let default = default.unwrap_or_default().to_string();
    App::new(move || {
        let value = signal(default.clone());
        let problem = signal(String::new());
        let validate = validate.clone();
        column([
            label(format!("[bold]? {}[/]", escape(&question))),
            input(value)
                .label(question.clone())
                .on_submit(move |typed, cx| match validate(typed) {
                    Ok(accepted) => cx.quit_with(accepted),
                    Err(message) => problem.set(message),
                })
                .error_when(move || !problem.get().is_empty()),
            text(move || {
                let message = problem.get();
                if message.is_empty() {
                    "[dim]Enter to accept · Esc to cancel[/]".to_string()
                } else {
                    format!("[red]{}[/]", escape(&message))
                }
            }),
        ])
        .on_key("esc", |cx| cx.quit())
    })
}

/// The step overview: every step with its mark, Enter to start.
pub fn overview_app(items: &[OverviewItem]) -> App {
    let titles: Vec<String> = items
        .iter()
        .map(|i| {
            if i.detail.is_empty() {
                escape(&i.title)
            } else {
                format!("{} [dim]— {}[/]", escape(&i.title), escape(&i.detail))
            }
        })
        .collect();
    let states: Vec<StepState> = items.iter().map(|i| i.state).collect();
    App::new(move || {
        let titles = titles.clone();
        let states = states.clone();
        column([
            label("[bold]Setting up the iPhone build[/]"),
            steps(
                move || titles.clone(),
                move |i| states.get(i).copied().unwrap_or_default(),
            ),
            label("[dim]Enter to start · Esc to stop[/]"),
        ])
        .on_key("enter", |cx| cx.quit_with(true))
        .on_key("esc", |cx| cx.quit_with(false))
    })
}

/// The terminal.
pub struct TermUi {
    console: Console,
    interactive: bool,
    assume_yes: bool,
}

impl TermUi {
    pub fn new(assume_yes: bool) -> TermUi {
        TermUi {
            console: make_console(),
            interactive: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            assume_yes,
        }
    }

    fn run_inline<T: 'static>(&self, app: App, rows: u16) -> Result<T> {
        app.inline(rows)
            .run_for::<T>()
            .map_err(|e| Error::Terminal(e.kind().to_string()))?
            .ok_or(Error::Cancelled)
    }

    fn line_prompt(&self, question: &str, default: Option<&str>) -> Result<String> {
        Prompt::new(format!("[bold]{}[/]", escape(question)))
            .ask(&self.console, default)
            .map_err(|e| Error::Terminal(e.kind().to_string()))
    }
}

impl Ui for TermUi {
    fn console(&self) -> &Console {
        &self.console
    }

    fn choose(&mut self, question: &str, options: &[String], default: usize) -> Result<usize> {
        if options.is_empty() {
            return Err(Error::Cancelled);
        }
        if self.interactive {
            let rows = choice_rows(options);
            return self.run_inline::<usize>(choice_app(question, options, default), rows);
        }
        // Without a terminal: a numbered list and rich's prompt.
        self.say(&format!("[bold]{}[/]", escape(question)));
        for (i, option) in options.iter().enumerate() {
            self.say(&format!("  {}: {}", i + 1, escape(option)));
        }
        let numbers: Vec<String> = (1..=options.len()).map(|n| n.to_string()).collect();
        let default_text = (default + 1).to_string();
        let answer = Prompt::new("Choice")
            .choices(numbers)
            .ask(&self.console, Some(&default_text))
            .map_err(|e| Error::Terminal(e.kind().to_string()))?;
        answer
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .ok_or(Error::Cancelled)
    }

    fn ask(
        &mut self,
        question: &str,
        default: Option<&str>,
        validate: Validator,
    ) -> Result<String> {
        if self.interactive {
            return self.run_inline::<String>(input_app(question, default, validate), 3);
        }
        loop {
            let answer = self.line_prompt(question, default)?;
            match validate(&answer) {
                Ok(value) => return Ok(value),
                Err(message) => self.say(&format!("[red]{}[/]", escape(&message))),
            }
        }
    }

    fn confirm(&mut self, question: &str, default: bool) -> Result<bool> {
        if self.assume_yes {
            self.say(&format!(
                "[bold]{}[/] {} [dim](--yes)[/]",
                escape(question),
                if default { "yes" } else { "no" }
            ));
            return Ok(default);
        }
        Confirm::new(format!("[bold]{}[/]", escape(question)))
            .ask(&self.console, Some(default))
            .map_err(|e| Error::Terminal(e.kind().to_string()))
    }

    fn overview(&mut self, items: &[OverviewItem]) -> Result<bool> {
        if self.interactive && !self.assume_yes {
            // A long detail can wrap onto a second row.
            let rows = u16::try_from(items.len() * 2 + 2).unwrap_or(u16::MAX);
            return self.run_inline::<bool>(overview_app(items), rows);
        }
        print_overview_table(self, items);
        Ok(true)
    }
}

pub fn print_overview_table(ui: &dyn Ui, items: &[OverviewItem]) {
    for item in items {
        let mark = match item.state {
            StepState::Done => "[green]✓[/]",
            StepState::Failed => "[red]✗[/]",
            StepState::Current => "▸",
            _ => "[dim]○[/]",
        };
        let detail = if item.detail.is_empty() {
            String::new()
        } else {
            format!(" [dim]— {}[/]", escape(&item.detail))
        };
        ui.say(&format!("{mark} {}{detail}", escape(&item.title)));
    }
}

#[cfg(test)]
pub mod testing {
    //! A UI for tests: the same intuiTUIve screens, driven headlessly with
    //! scripted keys, and everything printed captured as plain text.

    use std::collections::VecDeque;

    use super::*;

    #[derive(Debug, Clone)]
    pub enum Answer {
        /// Keys pressed on a choice screen (e.g. `"down enter"`).
        Keys(&'static str),
        /// Text typed into an input, then Enter.
        Text(String),
        /// A yes/no answer.
        Yes(bool),
    }

    pub struct ScriptUi {
        console: Console,
        answers: VecDeque<Answer>,
        transcript: String,
        /// The questions asked, in order.
        pub questions: Vec<String>,
    }

    impl ScriptUi {
        pub fn new(answers: impl IntoIterator<Item = Answer>) -> ScriptUi {
            let console = Console::builder()
                .emoji(false)
                .force_terminal(false)
                .color_system(None)
                .width(100)
                .build();
            console.begin_capture();
            ScriptUi {
                console,
                answers: answers.into_iter().collect(),
                transcript: String::new(),
                questions: Vec::new(),
            }
        }

        /// Everything printed and every screen shown, as plain text.
        pub fn transcript(&mut self) -> String {
            self.flush();
            self.transcript.clone()
        }

        fn flush(&mut self) {
            let printed = self.console.end_capture();
            self.transcript.push_str(&printed);
            self.console.begin_capture();
        }

        fn next(&mut self, question: &str) -> Answer {
            self.questions.push(question.to_string());
            self.answers
                .pop_front()
                .unwrap_or_else(|| panic!("no scripted answer for {question:?}"))
        }

        fn show(&mut self, rows: Vec<String>) {
            self.flush();
            for row in rows {
                self.transcript.push_str(row.trim_end());
                self.transcript.push('\n');
            }
        }
    }

    impl Ui for ScriptUi {
        fn console(&self) -> &Console {
            &self.console
        }

        fn choose(&mut self, question: &str, options: &[String], default: usize) -> Result<usize> {
            let Answer::Keys(keys) = self.next(question) else {
                panic!("expected keys for {question:?}");
            };
            let rows = choice_rows(options);
            let mut driver = choice_app(question, options, default).driver(100, rows);
            driver.settle();
            self.show(driver.screen().plain());
            driver.press(keys);
            driver.take_exit::<usize>().ok_or(Error::Cancelled)
        }

        fn ask(
            &mut self,
            question: &str,
            default: Option<&str>,
            validate: Validator,
        ) -> Result<String> {
            let Answer::Text(text) = self.next(question) else {
                panic!("expected text for {question:?}");
            };
            let mut driver = input_app(question, None, validate).driver(100, 3);
            let _ = default;
            driver.settle();
            driver.type_text(&text);
            driver.press("enter");
            let value = driver.take_exit::<String>();
            self.show(driver.screen().plain());
            value.ok_or_else(|| Error::InvalidInput(format!("rejected: {text}")))
        }

        fn confirm(&mut self, question: &str, _default: bool) -> Result<bool> {
            let Answer::Yes(yes) = self.next(question) else {
                panic!("expected yes/no for {question:?}");
            };
            self.say(&format!(
                "{} {}",
                escape(question),
                if yes { "y" } else { "n" }
            ));
            Ok(yes)
        }

        fn overview(&mut self, items: &[OverviewItem]) -> Result<bool> {
            print_overview_table(self, items);
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_screen_picks_with_the_arrows() {
        let options = vec!["First [x]".to_string(), "Second".to_string()];
        let mut driver = choice_app("Pick one", &options, 0).driver(40, 4);
        driver.settle();
        let rows = driver.screen().plain();
        assert_eq!(rows[1].trim_end(), "(•) First [x]");
        driver.press("down enter");
        assert_eq!(driver.take_exit::<usize>(), Some(1));
    }

    #[test]
    fn choice_screen_esc_cancels() {
        let options = vec!["A".to_string()];
        let mut driver = choice_app("Pick", &options, 0).driver(20, 3);
        driver.settle();
        driver.press("esc");
        assert!(driver.is_done());
        assert_eq!(driver.take_exit::<usize>(), None);
    }

    #[test]
    fn input_screen_shows_the_problem_until_valid() {
        let validate: Validator = Rc::new(|v: &str| crate::validate::key_id(v));
        let mut driver = input_app("Key ID", None, validate).driver(100, 3);
        driver.settle();
        driver.type_text("abc");
        driver.press("enter");
        assert_eq!(driver.take_exit::<String>(), None);
        assert!(driver.screen().plain()[2].contains("10 characters"));
        driver.press("ctrl+a");
        for _ in 0..3 {
            driver.press("delete");
        }
        driver.type_text("2X9R4HXF34");
        driver.press("enter");
        assert_eq!(driver.take_exit::<String>().as_deref(), Some("2X9R4HXF34"));
    }

    #[test]
    fn overview_lists_every_step() {
        let items = vec![
            OverviewItem {
                title: "WSL".into(),
                state: StepState::Done,
                detail: "Ubuntu 24.04".into(),
            },
            OverviewItem {
                title: "Swift".into(),
                state: StepState::Pending,
                detail: String::new(),
            },
        ];
        let mut driver = overview_app(&items).driver(60, 4);
        driver.settle();
        let rows = driver.screen().plain();
        assert_eq!(rows[1].trim_end(), "✓ WSL — Ubuntu 24.04");
        assert_eq!(rows[2].trim_end(), "○ Swift");
        driver.press("enter");
        assert_eq!(driver.take_exit::<bool>(), Some(true));
    }
}
