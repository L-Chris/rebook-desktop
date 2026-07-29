use crate::highlights::StoredHighlight;
use rebook_reader::{NavigationAttempt, NavigationOutcome, PageDirection};

use super::{DesktopReader, FollowUp, MarkRetention, ProgressChange, SidebarTab, SnapshotEffects};

impl DesktopReader {
    pub(in crate::reader) fn request_exit(&mut self) {
        self.persist_progress();
        self.exit_requested = true;
    }

    pub(in crate::reader) fn begin_text_selection(&mut self, x: f32, y: f32) {
        self.selection_toolbar_visible = false;
        self.annotation_note_draft = None;
        match self.reader.hit_test_current_spread(x, y, true) {
            Ok(anchor) => {
                self.selection_anchor = anchor;
                self.selection = None;
                self.selected_highlight_id = None;
                self.bump_scene_revision();
            }
            Err(error) => self.error = Some(format!("选择文字失败：{error}")),
        }
    }

    pub(in crate::reader) fn update_text_selection(&mut self, x: f32, y: f32) {
        let Some(anchor) = self.selection_anchor.clone() else {
            return;
        };
        let result = self
            .reader
            .hit_test_current_spread(x, y, false)
            .and_then(|focus| {
                focus.map_or(Ok(None), |focus| {
                    self.reader.selection_between(&anchor, &focus)
                })
            });
        match result {
            Ok(selection) if self.selection != selection => {
                self.selection = selection;
                self.bump_scene_revision();
            }
            Ok(_) => {}
            Err(error) => self.error = Some(format!("选择文字失败：{error}")),
        }
    }

    pub(in crate::reader) fn finish_text_selection(&mut self, x: f32, y: f32, moved: bool) {
        if moved {
            self.update_text_selection(x, y);
            if self.selection.is_none() {
                self.selection_anchor = None;
            }
            self.selection_toolbar_visible = self.selection.is_some();
            return;
        }

        self.selection_toolbar_visible = false;
        self.annotation_note_draft = None;
        self.selection_anchor = None;
        self.selection = None;
        self.bump_scene_revision();
        let candidates = self
            .highlights
            .iter()
            .map(|highlight| (highlight.id.clone(), highlight.ranges.clone()))
            .collect::<Vec<_>>();
        let activated = candidates.into_iter().find_map(|(id, ranges)| {
            self.reader
                .source_ranges_contain_point(&ranges, x, y)
                .ok()
                .filter(|contains| *contains)
                .map(|_| id)
        });
        if let Some(id) = activated {
            self.selected_highlight_id = Some(id);
            self.ui.sidebar_tab = SidebarTab::Highlights;
            self.set_sidebar_open(true);
        } else {
            self.selected_highlight_id = None;
        }
    }

    pub(in crate::reader) fn cancel_text_selection(&mut self) {
        self.selection_toolbar_visible = false;
        self.annotation_note_draft = None;
        self.selection_anchor = None;
        if self.selection.take().is_some() {
            self.bump_scene_revision();
        }
    }

    pub(in crate::reader) fn create_highlight(&mut self, note: Option<String>) {
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let highlight = StoredHighlight::with_note(
            self.book_id.clone(),
            selection.ranges,
            selection.text,
            note,
        );
        match self.highlight_store.insert(&highlight) {
            Ok(()) => {
                self.highlights.insert(0, highlight);
                self.selection_toolbar_visible = false;
                self.annotation_note_draft = None;
                self.selection_anchor = None;
                self.selection = None;
                self.selected_highlight_id = None;
                self.bump_scene_revision();
                self.error = None;
            }
            Err(error) => self.error = Some(format!("保存高亮失败：{error}")),
        }
    }

    pub(in crate::reader) fn remove_highlight(&mut self, id: &str) {
        match self.highlight_store.remove(id) {
            Ok(true) => {
                self.highlights.retain(|highlight| highlight.id != id);
                if self.selected_highlight_id.as_deref() == Some(id) {
                    self.selected_highlight_id = None;
                }
                self.bump_scene_revision();
                self.error = None;
            }
            Ok(false) => {}
            Err(error) => self.error = Some(format!("删除高亮失败：{error}")),
        }
    }

    pub(in crate::reader) fn go_to_highlight(&mut self, id: &str) {
        let Some(anchor) = self
            .highlights
            .iter()
            .find(|highlight| highlight.id == id)
            .and_then(|highlight| highlight.ranges.first())
            .map(|range| range.start.clone())
        else {
            return;
        };
        match self.reader.go_to_source(&anchor) {
            Ok(result) => {
                self.apply_snapshot(
                    result.snapshot,
                    SnapshotEffects {
                        marks: MarkRetention::Keep,
                        ..SnapshotEffects::navigation()
                    },
                );
                self.selected_highlight_id = Some(id.to_owned());
            }
            Err(error) => self.error = Some(format!("高亮跳转失败：{error}")),
        }
    }

    pub(in crate::reader) fn set_sidebar_tab(&mut self, tab: SidebarTab) {
        self.ui.sidebar_tab = tab;
    }

    pub(in crate::reader) fn turn_page(&mut self, direction: PageDirection) {
        if self.pending_page_turn.is_some() {
            return;
        }
        self.pending_page_turn = Some(direction);
        self.retry_pending_page_turn();
    }

    pub(in crate::reader) fn retry_pending_page_turn(&mut self) {
        let Some(direction) = self.pending_page_turn else {
            return;
        };
        let previous_section = self.snapshot.location.section_index;
        let previous_segment = self.snapshot.location.segment_index;
        let result = self.reader.try_turn_page(direction);
        if result.is_err() {
            self.pending_page_turn = None;
        }
        match result {
            Ok(NavigationAttempt::Pending) => {}
            Ok(NavigationAttempt::Ready(result)) => {
                let moved = result.outcome == NavigationOutcome::Moved;
                let section_changed = result.snapshot.location.section_index != previous_section;
                let segment_changed = result.snapshot.location.segment_index != previous_segment;
                self.apply_snapshot(
                    result.snapshot,
                    SnapshotEffects {
                        prefetch: if moved && (section_changed || segment_changed) {
                            FollowUp::Run
                        } else {
                            FollowUp::None
                        },
                        translation: if moved { FollowUp::Run } else { FollowUp::None },
                        progress: if moved {
                            ProgressChange::Persist
                        } else {
                            ProgressChange::Keep
                        },
                        ..SnapshotEffects::navigation()
                    },
                );
            }
            Err(error) => self.error = Some(format!("翻页失败：{error}")),
        }
    }
}
