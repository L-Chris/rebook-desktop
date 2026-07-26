#[derive(Clone)]
pub(crate) struct PendingTask<T> {
    pub id: u64,
    pub payload: T,
}

#[derive(Debug)]
pub(crate) struct TaskResult<T> {
    pub id: u64,
    pub result: Result<T, String>,
}

pub(crate) struct TaskSlot<T> {
    pub pending: Option<PendingTask<T>>,
    next_id: u64,
}

impl<T> Default for TaskSlot<T> {
    fn default() -> Self {
        Self {
            pending: None,
            next_id: 1,
        }
    }
}

impl<T> TaskSlot<T> {
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn begin(&mut self, payload: T) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.pending = Some(PendingTask { id, payload });
        id
    }

    pub fn complete(&mut self, id: u64) -> Option<T> {
        if self.pending.as_ref().map(|request| request.id) != Some(id) {
            return None;
        }
        self.pending.take().map(|request| request.payload)
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_completion_cannot_clear_the_current_request() {
        let mut slot = TaskSlot::default();
        let first = slot.begin("first");
        slot.cancel();
        let second = slot.begin("second");

        assert_eq!(slot.complete(first), None);
        assert!(slot.is_pending());
        assert_eq!(slot.complete(second), Some("second"));
        assert!(!slot.is_pending());
    }
}
