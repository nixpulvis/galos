//! The same queries, with somewhere to go next
//!
//! Nothing here knows what a system is. It asks [`Query`]s, draws [`View`]s,
//! and follows the [`Link`](crate::view::Link)s the views came with, which is
//! the entire reason a command can be added to `galos` without this file
//! being touched: a new command answers with a view like any other, and its
//! rows lead wherever it says they lead.
//!
//! The command bar is the CLI's own parser, so `search -s Sol -r 50` typed
//! here is `galos search -s Sol -r 50` typed at a shell, down to the error
//! message when it is typed wrong. And every row shows the command that would
//! have asked for it, which makes the interactive tool the fastest way to
//! learn the batch one.

mod render;

use crate::query::{Ask, Query};
use crate::view::{Fields, Section, Stop, View};
use crate::Result;
use async_std::channel::{self, Receiver};
use galos_db::Database;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};
use std::io;
use std::time::Duration;

/// How long the loop waits on a keypress before going round again
///
/// Short enough that an answer arriving from the database shows up as soon as
/// it lands, rather than the next time the user happens to press something,
/// and long enough that a terminal sitting idle is a process sitting idle.
const TICK: Duration = Duration::from_millis(100);

/// Take over the terminal until the user is done
pub fn run(db: Database) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let outcome = App::new(db).play(&mut terminal);
    ratatui::restore();
    outcome
}

/// Where the user is, and how they got there
struct App {
    db: Database,
    /// The pages behind the one being looked at, oldest first
    ///
    /// Never empty: the last of them is the page on screen, and going back
    /// from the only page is not going back.
    stack: Vec<Page>,
    /// The command being typed, if one is
    typing: Option<String>,
    /// A query asked and not yet answered
    asking: Option<Asking>,
    /// What went wrong with the last thing asked
    ///
    /// Shown against the page that was already there rather than replacing
    /// it. A typo in a command should not cost you the page you typed it
    /// from.
    trouble: Option<String>,
    ticks: usize,
    quit: bool,
}

/// One answer, and where the cursor is in it
struct Page {
    /// What was asked to get here, for asking again
    ///
    /// The help page has nothing to ask, which is the only reason this is an
    /// option.
    query: Option<Query>,
    view: View,
    stops: Vec<Stop>,
    cursor: usize,
    scroll: u16,
}

/// A query in flight
struct Asking {
    query: Query,
    answers: Receiver<Result<View>>,
    /// Whether the answer replaces the page it was asked from
    ///
    /// Asking again is not going somewhere new, and a stack that grows a page
    /// every time you press `r` is a stack you cannot get back out of.
    instead: bool,
}

impl App {
    fn new(db: Database) -> Self {
        App {
            db,
            stack: vec![Page::help()],
            typing: None,
            asking: None,
            trouble: None,
            ticks: 0,
            quit: false,
        }
    }

    fn play(mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.quit {
            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(TICK)? {
                if let Event::Key(key) = event::read()? {
                    // Windows reports the release as well as the press, and a
                    // key that moves the cursor twice per press is a cursor
                    // that skips every other row.
                    if key.kind == KeyEventKind::Press {
                        self.key(key);
                    }
                }
            }
            self.ticks += 1;
            self.collect();
        }
        Ok(())
    }

    /// The page on screen
    fn page(&self) -> &Page {
        self.stack.last().expect("a page to be looking at")
    }

    fn page_mut(&mut self) -> &mut Page {
        self.stack.last_mut().expect("a page to be looking at")
    }

    /// Take the answer, if one has arrived
    fn collect(&mut self) {
        let Some(asking) = &self.asking else { return };
        let Ok(answer) = asking.answers.try_recv() else { return };

        let asking = self.asking.take().expect("the query just answered");
        match answer {
            Ok(view) => {
                let page = Page::of(asking.query, view);
                if asking.instead {
                    // Where the cursor was is where it should stay: asking
                    // again is asking about the same thing, and a reload that
                    // sends you back to the top of a hundred bodies is a
                    // reload nobody presses twice.
                    let cursor = self.page().cursor;
                    let scroll = self.page().scroll;
                    let page = Page { cursor, scroll, ..page };
                    *self.page_mut() = page.settled();
                } else {
                    self.stack.push(page);
                }
                self.trouble = None;
            }
            Err(err) => self.trouble = Some(err.to_string()),
        }
    }

    /// Ask, and carry on drawing while it is answered
    ///
    /// Everything that reaches the database goes through here. A route across
    /// the bubble walks the graph for a good few seconds, and a UI that waits
    /// on it is a UI that cannot be told it was the wrong route.
    fn ask(&mut self, query: Query, instead: bool) {
        let (answered, answers) = channel::bounded(1);
        let db = self.db.clone();
        let asked = query.clone();
        async_std::task::spawn(async move {
            let _ = answered.send(asked.ask(&db).await).await;
        });
        self.asking = Some(Asking { query, answers, instead });
        self.trouble = None;
    }

    /// Go where the row under the cursor leads
    fn follow(&mut self) {
        let page = self.page();
        let Some(stop) = page.stops.get(page.cursor).copied() else { return };
        let Some(link) = page.view.link(stop) else { return };
        let query = link.query().clone();
        self.ask(query, false);
    }

    /// Back to the page this one was reached from
    fn back(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
            self.trouble = None;
        }
    }

    /// Ask the current page's query again
    fn reload(&mut self) {
        if let Some(query) = self.page().query.clone() {
            self.ask(query, true);
        }
    }

    fn key(&mut self, key: KeyEvent) {
        if self.typing.is_some() {
            return self.typed(key);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char(':') | KeyCode::Char('/') => {
                self.typing = Some(String::new())
            }
            KeyCode::Char('?') => self.stack.push(Page::help()),
            KeyCode::Char('r') => self.reload(),

            KeyCode::Char('j') | KeyCode::Down => self.step(1),
            KeyCode::Char('k') | KeyCode::Up => self.step(-1),
            KeyCode::PageDown => self.step(10),
            KeyCode::PageUp => self.step(-10),
            KeyCode::Home | KeyCode::Char('g') => self.page_mut().cursor = 0,
            KeyCode::End | KeyCode::Char('G') => {
                let last = self.page().stops.len().saturating_sub(1);
                self.page_mut().cursor = last;
            }

            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                self.follow()
            }
            KeyCode::Esc
            | KeyCode::Backspace
            | KeyCode::Char('h')
            | KeyCode::Left => self.back(),

            _ => {}
        }
    }

    /// A keypress while the command bar is open
    fn typed(&mut self, key: KeyEvent) {
        let Some(line) = &mut self.typing else { return };
        match key.code {
            KeyCode::Esc => self.typing = None,
            KeyCode::Backspace => {
                if line.pop().is_none() {
                    // Backspacing out of an empty line puts the bar away,
                    // which is where backspacing from the start of a line
                    // goes everywhere else.
                    self.typing = None;
                }
            }
            KeyCode::Enter => {
                let line = self.typing.take().unwrap_or_default();
                match Query::parse_line(&line) {
                    Ok(query) => self.ask(query, false),
                    Err(err) => self.trouble = Some(err.to_string()),
                }
            }
            KeyCode::Char(c) => line.push(c),
            _ => {}
        }
    }

    /// Move the cursor `by` rows, stopping at either end
    fn step(&mut self, by: isize) {
        let last = self.page().stops.len().saturating_sub(1);
        let cursor = self.page().cursor as isize + by;
        self.page_mut().cursor = cursor.clamp(0, last as isize) as usize;
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [top, body, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        frame.render_widget(self.breadcrumbs(top.width), top);
        self.draw_page(frame, body);
        frame.render_widget(self.status(bottom.width), bottom);

        // The block cursor belongs in the command bar when there is one, so
        // that a terminal's own idea of where you are typing agrees with the
        // page's.
        if let Some(line) = &self.typing {
            let at = 2 + line.chars().count() as u16;
            frame.set_cursor_position((bottom.x + at, bottom.y));
        }
    }

    fn draw_page(&mut self, frame: &mut Frame, area: Rect) {
        let page = self.page();
        let at = page.stops.get(page.cursor).copied();
        let drawn = render::render(&page.view, at, area.width);

        // Scrolling is settled here rather than when the cursor moves,
        // because how many lines a row is down the page depends on how wide
        // the terminal is, and that is not known until it is being drawn.
        let height = area.height as usize;
        if let Some(line) = drawn.cursor {
            let scroll = page.scroll as usize;
            let settled = if line < scroll {
                line
            } else if line >= scroll + height {
                line + 1 - height
            } else {
                scroll
            };
            self.page_mut().scroll = settled as u16;
        }

        let scroll = self.page().scroll;
        frame.render_widget(
            Paragraph::new(drawn.lines).scroll((scroll, 0)),
            area,
        );
    }

    /// The pages behind this one, and what the keys are
    fn breadcrumbs(&self, width: u16) -> Paragraph<'static> {
        let trail = self
            .stack
            .iter()
            .map(|page| page.view.title.as_str())
            .collect::<Vec<_>>()
            .join(" › ");
        let keys = "? keys   q quit";

        // A space in front, two between, and the keys against the right edge.
        let room = (width as usize).saturating_sub(keys.len() + 3);
        let trail = if trail.chars().count() > room {
            // The end of the trail is where the user is, so it is the end
            // that is kept.
            let cut = trail.chars().count() + 1 - room;
            format!("…{}", trail.chars().skip(cut).collect::<String>())
        } else {
            trail
        };

        let gap = (width as usize)
            .saturating_sub(trail.chars().count() + keys.len() + 1)
            .max(1);
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {trail}"),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".repeat(gap)),
            Span::styled(keys, Style::new().dim()),
        ]))
    }

    /// The one line at the bottom, saying whichever of four things is true
    fn status(&self, width: u16) -> Paragraph<'static> {
        if let Some(line) = &self.typing {
            return Paragraph::new(Line::from(vec![
                Span::styled(": ", Style::new().dim()),
                Span::raw(line.clone()),
            ]));
        }

        if let Some(asking) = &self.asking {
            let spin = ["|", "/", "-", "\\"][self.ticks / 2 % 4];
            return Paragraph::new(Line::styled(
                format!(" {spin} {}", asking.query.command()),
                Style::new().yellow(),
            ));
        }

        if let Some(trouble) = &self.trouble {
            return Paragraph::new(Line::styled(
                format!(" {trouble}"),
                Style::new().red(),
            ));
        }

        // Nothing is happening, so the bar says what the page came to and
        // what pressing enter would ask. The second half is the whole of the
        // CLI's discoverability: the command for wherever you are about to go
        // is on the screen before you go there.
        let page = self.page();
        let note = page.view.note.clone().unwrap_or_default();
        let under = page
            .stops
            .get(page.cursor)
            .and_then(|stop| page.view.link(*stop))
            .map(|link| link.command())
            .unwrap_or_default();

        let room = (width as usize).saturating_sub(note.chars().count() + 4);
        let under =
            if under.chars().count() > room { String::new() } else { under };

        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {note}"), Style::new().dim()),
            Span::raw(" ".repeat((width as usize).saturating_sub(
                note.chars().count() + under.chars().count() + 2,
            ))),
            Span::styled(under, Style::new().cyan()),
        ]))
    }
}

impl Page {
    fn of(query: Query, view: View) -> Self {
        Page {
            query: Some(query),
            stops: view.stops(),
            view,
            cursor: 0,
            scroll: 0,
        }
    }

    /// The same page with the cursor put back inside it
    ///
    /// A reload can come back with fewer rows than it went out with, and a
    /// cursor kept from the old answer can be past the end of the new one.
    fn settled(mut self) -> Self {
        self.cursor = self.cursor.min(self.stops.len().saturating_sub(1));
        self
    }

    /// What there is to press, and what there is to ask
    ///
    /// Built as a [`View`] like everything else, so it is drawn by the same
    /// code and scrolls the same way. The commands are read out of the same
    /// derived grammar the parser uses, so a command added to [`Query`] is
    /// listed here without anybody remembering to list it.
    fn help() -> Self {
        let mut commands = Fields::new();
        for (name, about) in Query::summaries() {
            commands = commands.and(name, about);
        }

        let view = View::new("galos")
            .with(Section::Note("Keys".into()))
            .with(
                Fields::new()
                    .and("j k ↑ ↓", "move between rows")
                    .and("enter →", "follow the row under the cursor")
                    .and("esc ← backspace", "back to the page before")
                    .and(":", "type a command")
                    .and("r", "ask this page again")
                    .and("?", "this page")
                    .and("q", "quit"),
            )
            .with(Section::Note("Commands".into()))
            .with(commands)
            .with(Section::Note(
                "Typed here or passed to `galos` at a shell, they are the \
                 same commands: `search -s Sol -r 50`."
                    .into(),
            ))
            .noting("press : to ask something");

        Page { query: None, stops: view.stops(), view, cursor: 0, scroll: 0 }
    }
}
