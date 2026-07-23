//! Explicit reader command state machine with cancellation and stale-result rejection.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rebook_publication::{LocatorV1, PublicationId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Cooperative task cancellation shared with parser and layout workers.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// Requests cancellation.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Reader lifecycle visible to the desktop shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReaderState {
    /// No publication is open.
    Idle,
    /// A parser task is opening a publication.
    Opening,
    /// Publication metadata and reading order are available.
    Ready,
    /// Content is being styled or laid out.
    LayingOut,
    /// A surface is ready for display.
    Displaying,
    /// The most recent active task failed.
    Error,
}

/// Logical viewport used to invalidate layout.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    /// Logical width in device-independent pixels.
    pub width: f32,
    /// Logical height in device-independent pixels.
    pub height: f32,
    /// Device scale factor.
    pub scale_factor: f32,
}

impl Viewport {
    /// Validates finite positive dimensions and scale.
    pub fn validate(self) -> Result<Self, ReaderError> {
        if self.width.is_finite()
            && self.height.is_finite()
            && self.scale_factor.is_finite()
            && self.width > 0.0
            && self.height > 0.0
            && self.scale_factor > 0.0
        {
            Ok(self)
        } else {
            Err(ReaderError::InvalidViewport)
        }
    }
}

/// Durable preferences whose changes can trigger reflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReaderPreferences {
    /// Base font size in logical pixels.
    pub font_size: f32,
    /// Unitless line-height multiplier.
    pub line_height: f32,
    /// Reader page or viewport margin in logical pixels.
    pub margin: f32,
    /// Optional user-selected font family.
    pub font_family: Option<String>,
    /// Reader color scheme.
    pub color_scheme: ColorScheme,
    /// Continuous or paginated flow.
    pub flow: ReaderFlow,
}

impl Default for ReaderPreferences {
    fn default() -> Self {
        Self {
            font_size: 18.0,
            line_height: 1.6,
            margin: 32.0,
            font_family: None,
            color_scheme: ColorScheme::Light,
            flow: ReaderFlow::Scrolled,
        }
    }
}

/// Reader color scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorScheme {
    /// Light paper and dark text.
    Light,
    /// Dark paper and light text.
    Dark,
    /// Warm low-contrast paper.
    Sepia,
}

/// Layout flow selected by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReaderFlow {
    /// Continuous vertical flow.
    Scrolled,
    /// Fragmented pages.
    Paginated,
}

/// Commands accepted from the desktop shell.
#[derive(Debug, Clone, PartialEq)]
pub enum ReaderCommand {
    /// Start opening a new publication.
    Open { display_name: String },
    /// Close the current publication and cancel all work.
    Close,
    /// Navigate to a durable location.
    GoTo { locator: Box<LocatorV1> },
    /// Change viewport geometry.
    SetViewport(Viewport),
    /// Replace all layout-affecting preferences.
    SetPreferences(ReaderPreferences),
}

/// Reason for a layout task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutReason {
    /// First content after opening.
    Initial,
    /// Explicit navigation.
    Navigation,
    /// Viewport size or DPI changed.
    Viewport,
    /// Reader preferences changed.
    Preferences,
}

/// Work item that the controller delegates to a worker pool.
#[derive(Debug, Clone)]
pub enum ReaderTask {
    /// Detect and parse a publication.
    Open {
        /// Generation that must be supplied when completing the task.
        generation: u64,
        /// Display name for diagnostics.
        display_name: String,
        /// Cooperative cancellation token.
        cancellation: CancellationToken,
    },
    /// Produce a new surface for the current location.
    Layout {
        /// Generation that must be supplied when completing the task.
        generation: u64,
        /// Why layout was scheduled.
        reason: LayoutReason,
        /// Target location.
        locator: Box<LocatorV1>,
        /// Current viewport, if already known.
        viewport: Option<Viewport>,
        /// Preferences snapshot for deterministic layout.
        preferences: ReaderPreferences,
        /// Cooperative cancellation token.
        cancellation: CancellationToken,
    },
}

/// Observable events produced by the controller.
#[derive(Debug, Clone, PartialEq)]
pub enum ReaderEvent {
    /// Lifecycle state changed.
    StateChanged(ReaderState),
    /// Publication metadata became available.
    PublicationOpened { publication_id: PublicationId },
    /// A display surface was committed at this locator.
    Relocated { locator: Box<LocatorV1> },
    /// Active work failed.
    Failed { message: String },
    /// Publication was closed.
    Closed,
}

/// Result of dispatching a command.
#[derive(Debug, Clone, Default)]
pub struct CommandOutcome {
    /// Events that should be broadcast immediately.
    pub events: Vec<ReaderEvent>,
    /// Optional worker task to enqueue.
    pub task: Option<ReaderTask>,
}

/// Immutable state exposed to UI render code.
#[derive(Debug, Clone, PartialEq)]
pub struct ReaderSnapshot {
    /// Current lifecycle state.
    pub state: ReaderState,
    /// Monotonic active generation.
    pub generation: u64,
    /// Currently open publication.
    pub publication_id: Option<PublicationId>,
    /// Last committed or requested locator.
    pub locator: Option<LocatorV1>,
    /// Current viewport.
    pub viewport: Option<Viewport>,
    /// Current reader preferences.
    pub preferences: ReaderPreferences,
}

/// Single-owner reader state machine.
#[derive(Debug)]
pub struct ReaderController {
    snapshot: ReaderSnapshot,
    active_cancellation: Option<CancellationToken>,
}

impl Default for ReaderController {
    fn default() -> Self {
        Self::new()
    }
}

impl ReaderController {
    /// Creates an idle reader.
    pub fn new() -> Self {
        Self {
            snapshot: ReaderSnapshot {
                state: ReaderState::Idle,
                generation: 0,
                publication_id: None,
                locator: None,
                viewport: None,
                preferences: ReaderPreferences::default(),
            },
            active_cancellation: None,
        }
    }

    /// Returns an immutable UI snapshot.
    pub fn snapshot(&self) -> &ReaderSnapshot {
        &self.snapshot
    }

    /// Applies one UI command and optionally schedules worker work.
    pub fn dispatch(&mut self, command: ReaderCommand) -> Result<CommandOutcome, ReaderError> {
        match command {
            ReaderCommand::Open { display_name } => Ok(self.begin_open(display_name)),
            ReaderCommand::Close => Ok(self.close()),
            ReaderCommand::GoTo { locator } => self.go_to(*locator),
            ReaderCommand::SetViewport(viewport) => self.set_viewport(viewport),
            ReaderCommand::SetPreferences(preferences) => self.set_preferences(preferences),
        }
    }

    /// Commits a parser result if it still belongs to the active generation.
    pub fn complete_open(
        &mut self,
        generation: u64,
        publication_id: PublicationId,
        initial_locator: LocatorV1,
    ) -> Result<CommandOutcome, ReaderError> {
        if !self.is_active(generation) {
            return Ok(CommandOutcome::default());
        }
        initial_locator.validate()?;
        if initial_locator.publication_id != publication_id {
            return Err(ReaderError::LocatorPublicationMismatch);
        }

        self.snapshot.publication_id = Some(publication_id.clone());
        self.snapshot.locator = Some(initial_locator.clone());
        self.snapshot.state = ReaderState::LayingOut;
        let viewport = self.snapshot.viewport;
        let preferences = self.snapshot.preferences.clone();
        let task = self.replace_task(move |generation, cancellation| ReaderTask::Layout {
            generation,
            reason: LayoutReason::Initial,
            locator: Box::new(initial_locator),
            viewport,
            preferences,
            cancellation,
        });
        Ok(CommandOutcome {
            events: vec![
                ReaderEvent::PublicationOpened { publication_id },
                ReaderEvent::StateChanged(ReaderState::LayingOut),
            ],
            task: Some(task),
        })
    }

    /// Commits a layout result if it still belongs to the active generation.
    pub fn complete_layout(
        &mut self,
        generation: u64,
        locator: LocatorV1,
    ) -> Result<CommandOutcome, ReaderError> {
        if !self.is_active(generation) {
            return Ok(CommandOutcome::default());
        }
        self.validate_locator(&locator)?;
        self.active_cancellation = None;
        self.snapshot.locator = Some(locator.clone());
        self.snapshot.state = ReaderState::Displaying;
        Ok(CommandOutcome {
            events: vec![
                ReaderEvent::Relocated {
                    locator: Box::new(locator),
                },
                ReaderEvent::StateChanged(ReaderState::Displaying),
            ],
            task: None,
        })
    }

    /// Fails the active generation while silently ignoring stale results.
    pub fn fail_task(&mut self, generation: u64, message: impl Into<String>) -> CommandOutcome {
        if !self.is_active(generation) {
            return CommandOutcome::default();
        }
        self.active_cancellation = None;
        self.snapshot.state = ReaderState::Error;
        CommandOutcome {
            events: vec![
                ReaderEvent::Failed {
                    message: message.into(),
                },
                ReaderEvent::StateChanged(ReaderState::Error),
            ],
            task: None,
        }
    }

    fn begin_open(&mut self, display_name: String) -> CommandOutcome {
        self.cancel_active();
        self.snapshot.generation = self.snapshot.generation.saturating_add(1);
        self.snapshot.state = ReaderState::Opening;
        self.snapshot.publication_id = None;
        self.snapshot.locator = None;
        let cancellation = CancellationToken::default();
        self.active_cancellation = Some(cancellation.clone());
        CommandOutcome {
            events: vec![ReaderEvent::StateChanged(ReaderState::Opening)],
            task: Some(ReaderTask::Open {
                generation: self.snapshot.generation,
                display_name,
                cancellation,
            }),
        }
    }

    fn close(&mut self) -> CommandOutcome {
        self.cancel_active();
        self.snapshot.generation = self.snapshot.generation.saturating_add(1);
        self.snapshot.state = ReaderState::Idle;
        self.snapshot.publication_id = None;
        self.snapshot.locator = None;
        CommandOutcome {
            events: vec![
                ReaderEvent::Closed,
                ReaderEvent::StateChanged(ReaderState::Idle),
            ],
            task: None,
        }
    }

    fn go_to(&mut self, locator: LocatorV1) -> Result<CommandOutcome, ReaderError> {
        self.validate_locator(&locator)?;
        self.snapshot.locator = Some(locator.clone());
        Ok(self.schedule_layout(locator, LayoutReason::Navigation))
    }

    fn set_viewport(&mut self, viewport: Viewport) -> Result<CommandOutcome, ReaderError> {
        let viewport = viewport.validate()?;
        if self.snapshot.viewport == Some(viewport) {
            return Ok(CommandOutcome::default());
        }
        self.snapshot.viewport = Some(viewport);
        Ok(self
            .snapshot
            .locator
            .clone()
            .map_or_else(CommandOutcome::default, |locator| {
                self.schedule_layout(locator, LayoutReason::Viewport)
            }))
    }

    fn set_preferences(
        &mut self,
        preferences: ReaderPreferences,
    ) -> Result<CommandOutcome, ReaderError> {
        validate_preferences(&preferences)?;
        if self.snapshot.preferences == preferences {
            return Ok(CommandOutcome::default());
        }
        self.snapshot.preferences = preferences;
        Ok(self
            .snapshot
            .locator
            .clone()
            .map_or_else(CommandOutcome::default, |locator| {
                self.schedule_layout(locator, LayoutReason::Preferences)
            }))
    }

    fn schedule_layout(&mut self, locator: LocatorV1, reason: LayoutReason) -> CommandOutcome {
        self.snapshot.state = ReaderState::LayingOut;
        let viewport = self.snapshot.viewport;
        let preferences = self.snapshot.preferences.clone();
        let task = self.replace_task(move |generation, cancellation| ReaderTask::Layout {
            generation,
            reason,
            locator: Box::new(locator),
            viewport,
            preferences,
            cancellation,
        });
        CommandOutcome {
            events: vec![ReaderEvent::StateChanged(ReaderState::LayingOut)],
            task: Some(task),
        }
    }

    fn replace_task(
        &mut self,
        build: impl FnOnce(u64, CancellationToken) -> ReaderTask,
    ) -> ReaderTask {
        self.cancel_active();
        self.snapshot.generation = self.snapshot.generation.saturating_add(1);
        let cancellation = CancellationToken::default();
        self.active_cancellation = Some(cancellation.clone());
        build(self.snapshot.generation, cancellation)
    }

    fn validate_locator(&self, locator: &LocatorV1) -> Result<(), ReaderError> {
        locator.validate()?;
        let Some(publication_id) = &self.snapshot.publication_id else {
            return Err(ReaderError::NoPublication);
        };
        if &locator.publication_id != publication_id {
            return Err(ReaderError::LocatorPublicationMismatch);
        }
        Ok(())
    }

    fn is_active(&self, generation: u64) -> bool {
        self.snapshot.generation == generation
            && self
                .active_cancellation
                .as_ref()
                .is_some_and(|token| !token.is_cancelled())
    }

    fn cancel_active(&mut self) {
        if let Some(token) = self.active_cancellation.take() {
            token.cancel();
        }
    }
}

/// Reader state-machine validation errors.
#[derive(Debug, Error)]
pub enum ReaderError {
    /// Navigation was requested before a publication was opened.
    #[error("no publication is open")]
    NoPublication,
    /// A locator belongs to another publication.
    #[error("locator belongs to a different publication")]
    LocatorPublicationMismatch,
    /// Viewport dimensions or scale were not finite and positive.
    #[error("viewport dimensions and scale must be finite and positive")]
    InvalidViewport,
    /// One or more reader preferences were outside supported bounds.
    #[error("reader preferences are invalid")]
    InvalidPreferences,
    /// Publication model validation failed.
    #[error(transparent)]
    Publication(#[from] rebook_publication::PublicationError),
}

fn validate_preferences(preferences: &ReaderPreferences) -> Result<(), ReaderError> {
    if preferences.font_size.is_finite()
        && preferences.line_height.is_finite()
        && preferences.margin.is_finite()
        && (6.0..=144.0).contains(&preferences.font_size)
        && (0.8..=4.0).contains(&preferences.line_height)
        && (0.0..=512.0).contains(&preferences.margin)
    {
        Ok(())
    } else {
        Err(ReaderError::InvalidPreferences)
    }
}

#[cfg(test)]
mod tests {
    use rebook_publication::{LocatorV1, PublicationId, PublicationUrl};

    use super::{LayoutReason, ReaderCommand, ReaderController, ReaderState, ReaderTask, Viewport};

    fn publication_id(value: &str) -> PublicationId {
        PublicationId::new(value).expect("valid publication ID")
    }

    fn locator(publication_id: PublicationId) -> LocatorV1 {
        LocatorV1::at_start(
            publication_id,
            PublicationUrl::parse("OPS/chapter.xhtml").expect("valid URL"),
        )
    }

    #[test]
    fn a_new_open_cancels_the_previous_task_and_rejects_stale_completion() {
        let mut reader = ReaderController::new();
        let first = reader
            .dispatch(ReaderCommand::Open {
                display_name: "first.epub".into(),
            })
            .expect("open command");
        let ReaderTask::Open {
            generation: first_generation,
            cancellation: first_cancellation,
            ..
        } = first.task.expect("open task")
        else {
            panic!("expected open task");
        };

        let second = reader
            .dispatch(ReaderCommand::Open {
                display_name: "second.epub".into(),
            })
            .expect("second open command");
        assert!(first_cancellation.is_cancelled());
        assert!(matches!(second.task, Some(ReaderTask::Open { .. })));

        let stale = reader
            .complete_open(
                first_generation,
                publication_id("first"),
                locator(publication_id("first")),
            )
            .expect("stale completion is ignored");
        assert!(stale.events.is_empty());
        assert_eq!(reader.snapshot().state, ReaderState::Opening);
    }

    #[test]
    fn open_completion_schedules_initial_layout() {
        let mut reader = ReaderController::new();
        let open = reader
            .dispatch(ReaderCommand::Open {
                display_name: "book.epub".into(),
            })
            .expect("open command");
        let ReaderTask::Open { generation, .. } = open.task.expect("open task") else {
            panic!("expected open task");
        };
        let id = publication_id("book");
        let completed = reader
            .complete_open(generation, id.clone(), locator(id))
            .expect("complete open");

        assert_eq!(reader.snapshot().state, ReaderState::LayingOut);
        assert!(matches!(
            completed.task,
            Some(ReaderTask::Layout {
                reason: LayoutReason::Initial,
                ..
            })
        ));
    }

    #[test]
    fn viewport_change_replaces_an_active_layout_generation() {
        let mut reader = ReaderController::new();
        let open = reader
            .dispatch(ReaderCommand::Open {
                display_name: "book.epub".into(),
            })
            .expect("open command");
        let ReaderTask::Open { generation, .. } = open.task.expect("open task") else {
            panic!("expected open task");
        };
        let id = publication_id("book");
        let initial = reader
            .complete_open(generation, id.clone(), locator(id))
            .expect("complete open");
        let ReaderTask::Layout {
            cancellation: initial_cancellation,
            ..
        } = initial.task.expect("layout task")
        else {
            panic!("expected layout task");
        };

        let resized = reader
            .dispatch(ReaderCommand::SetViewport(Viewport {
                width: 1200.0,
                height: 800.0,
                scale_factor: 2.0,
            }))
            .expect("viewport command");
        assert!(initial_cancellation.is_cancelled());
        assert!(matches!(
            resized.task,
            Some(ReaderTask::Layout {
                reason: LayoutReason::Viewport,
                ..
            })
        ));
    }

    #[test]
    fn rejects_locator_from_another_publication() {
        let mut reader = ReaderController::new();
        let open = reader
            .dispatch(ReaderCommand::Open {
                display_name: "book.epub".into(),
            })
            .expect("open command");
        let ReaderTask::Open { generation, .. } = open.task.expect("open task") else {
            panic!("expected open task");
        };
        let id = publication_id("book");
        reader
            .complete_open(generation, id.clone(), locator(id))
            .expect("complete open");

        let result = reader.dispatch(ReaderCommand::GoTo {
            locator: Box::new(locator(publication_id("different"))),
        });
        assert!(result.is_err());
    }
}
