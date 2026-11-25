//! Acknowledgements belong to their initiating thread, not the selected view.
use crate::{git::record::Uncovered, host::UndoTurnReply, ownership::ThreadKey, state::CheckoutId};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct UndoAttempt {
    pub run: String,
    pub target: String,
    pub request: String,
    pub leave_changed: bool,
}

#[derive(Clone, Debug)]
pub struct UndoFailure {
    /// `None` means transport uncertainty: a retry must keep the original ID.
    pub code: Option<String>,
    pub message: String,
}

#[derive(Clone)]
pub struct UndoOperation {
    pub checkout: CheckoutId,
    pub attempt: UndoAttempt,
    pub coverage: Vec<Uncovered>,
    pub sending: bool,
    pub result: Option<Result<UndoTurnReply, UndoFailure>>,
}

#[derive(Default)]
pub struct UndoLedger {
    operations: HashMap<ThreadKey, UndoOperation>,
}

impl UndoLedger {
    pub fn begin(
        &mut self,
        owner: ThreadKey,
        checkout: CheckoutId,
        attempt: UndoAttempt,
        coverage: Vec<Uncovered>,
    ) -> bool {
        if self.operations.get(&owner).is_some_and(|op| op.sending) {
            return false;
        }
        self.operations.insert(
            owner,
            UndoOperation {
                checkout,
                attempt,
                coverage,
                sending: true,
                result: None,
            },
        );
        true
    }

    pub fn finish(
        &mut self,
        owner: &ThreadKey,
        request: &str,
        result: Result<UndoTurnReply, UndoFailure>,
    ) -> bool {
        let Some(operation) = self
            .operations
            .get_mut(owner)
            .filter(|op| op.sending && op.attempt.request == request)
        else {
            return false;
        };
        operation.sending = false;
        operation.result = Some(result);
        true
    }

    pub fn dismiss(&mut self, owner: &ThreadKey) {
        if self.operations.get(owner).is_some_and(|op| !op.sending) {
            self.operations.remove(owner);
        }
    }

    pub fn get(&self, owner: &ThreadKey) -> Option<&UndoOperation> {
        self.operations.get(owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_acknowledgements_keep_owner_and_uncertain_request() {
        let a = ThreadKey {
            host: None,
            id: "a".into(),
        };
        let b = ThreadKey {
            host: None,
            id: "b".into(),
        };
        let attempt = UndoAttempt {
            run: "run".into(),
            target: "record".into(),
            request: "request".into(),
            leave_changed: false,
        };
        let mut ledger = UndoLedger::default();
        assert!(ledger.begin(a.clone(), CheckoutId::default(), attempt.clone(), vec![]));
        assert!(!ledger.begin(a.clone(), CheckoutId::default(), attempt.clone(), vec![]));
        let failure = || {
            Err(UndoFailure {
                code: None,
                message: "lost reply".into(),
            })
        };
        assert!(!ledger.finish(&b, "request", failure()));
        assert!(!ledger.finish(&a, "different", failure()));
        assert!(ledger.finish(&a, "request", failure()));
        assert!(!ledger.finish(&a, "request", failure()));
        assert!(ledger.get(&b).is_none());
        let retry = ledger.get(&a).unwrap().attempt.clone();
        assert_eq!(retry.request, attempt.request);
        assert!(ledger.begin(a.clone(), CheckoutId::default(), retry, vec![]));
        assert!(ledger.finish(&a, "request", failure()));
    }
}
