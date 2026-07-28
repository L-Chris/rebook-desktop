use rebook_layout::LayoutViewport;
use rebook_publication::PublicationUrl;
use rebook_reader::ReaderSnapshot;

use super::{
    DesktopReader, FollowUp, MarkRetention, ProgressChange, SceneChange, SnapshotEffects,
    logical_dimension,
};

impl DesktopReader {
    pub(in crate::reader) fn go_to(&mut self, target: &PublicationUrl) {
        let result = self.reader.go_to_href(target);
        match result {
            Ok(result) => {
                self.apply_snapshot(result.snapshot, SnapshotEffects::navigation());
            }
            Err(error) => self.error = Some(format!("目录跳转失败：{error}")),
        }
    }

    pub(in crate::reader) fn resize_canvas(&mut self, width: f64, height: f64) {
        let width = logical_dimension(width);
        let height = logical_dimension(height);
        if width == 0 || height == 0 || self.canvas_size == Some((width, height)) {
            return;
        }
        if self.ui.sidebar_motion.is_animating() || self.ui.assistant_motion.is_animating() {
            return;
        }
        let Ok(viewport) = LayoutViewport::new(width, height) else {
            return;
        };
        let result = self.reader.resize(viewport);
        match result {
            Ok(snapshot) => {
                self.canvas_size = Some((width, height));
                self.apply_snapshot(snapshot, SnapshotEffects::static_content_change());
            }
            Err(error) => self.error = Some(format!("调整页面失败：{error}")),
        }
    }

    pub(in crate::reader) fn prefetch(&mut self) {
        let result = self
            .reader
            .prefetch_adjacent()
            .err()
            .map(|error| format!("章节预取失败：{error}"));
        self.error = result;
    }

    pub(in crate::reader) fn toggle_toc(&mut self, id: &str) {
        if !self.ui.expanded_toc.remove(id) {
            self.ui.expanded_toc.insert(id.to_owned());
        }
    }

    pub(in crate::reader) fn install_snapshot(&mut self, snapshot: ReaderSnapshot) {
        self.ui
            .expanded_toc
            .extend(snapshot.active_toc_path.iter().cloned());
        self.snapshot = snapshot;
    }

    pub(in crate::reader) fn apply_snapshot(
        &mut self,
        snapshot: ReaderSnapshot,
        effects: SnapshotEffects,
    ) {
        self.pending_page_turn = None;
        self.install_snapshot(snapshot);
        self.selection_toolbar_visible = false;
        self.selection_anchor = None;
        self.selection = None;
        match effects.marks {
            MarkRetention::Keep => {}
            MarkRetention::ClearSelectedHighlight => self.selected_highlight_id = None,
            MarkRetention::ClearAll => {
                self.selected_highlight_id = None;
                self.focused_mark = None;
            }
        }
        match effects.scene {
            SceneChange::Overlays => self.bump_scene_revision(),
            SceneChange::StaticContent => self.invalidate_page_scenes(),
        }
        self.error = None;
        if matches!(effects.progress, ProgressChange::Persist) {
            self.persist_progress();
        }
        if matches!(effects.prefetch, FollowUp::Run) {
            self.prefetch();
        }
        if matches!(effects.translation, FollowUp::Run) {
            self.queue_visible_section_translation();
        }
    }

    pub(in crate::reader) fn persist_progress(&self) {
        let Some(store) = &self.progress_store else {
            return;
        };
        let locator = self.reader.current_locator();
        if let Err(error) = store.save_progress(&self.book_id, &locator) {
            tracing::warn!(%error, book_id = %self.book_id, "failed to persist reading progress");
        }
    }

    pub(in crate::reader) fn progress(&self) -> f64 {
        self.snapshot.total_progression
    }
}
