//! Answer outcome settlement for [`ConversationSurface`] gates.
//!
//! Transport answer outcomes settle exactly the row gate that dispatched the
//! attempt. Correlation is by command identity: the gate's latest request
//! identity must still name the command carried by the outcome, so a stale or
//! unmatched receipt/failure settles nothing and can never corrupt a retried
//! flight. A settled receipt keeps the gate closed until the row resolves in
//! place; a failure reopens it with the existing pairing message, which the
//! row renders through the existing failure path.

use super::*;

impl ConversationSurface {
    /// Settles one recorded approval against its row gate.
    ///
    /// Returns the pairing when the gate's latest attempt still names
    /// `command`; an unknown row or a superseded attempt returns `None`
    /// without touching any gate.
    pub fn settle_approval_answered(
        &mut self,
        state: &EngineObservationState,
        command: &artisan_domain::RespondApproval,
        receipt: &RespondApprovalReceipt,
        cx: &mut Context<Self>,
    ) -> Option<AnswerPairing> {
        let gate = self
            .approval_gates
            .get_mut(command.approval_id().as_str())?;
        if gate.last_request_id() != Some(command.request_id()) {
            return None;
        }
        let pairing = gate.settle_receipt(state, command, receipt);
        cx.notify();
        Some(pairing)
    }

    /// Settles one failed approval against its row gate.
    ///
    /// The gate reopens with the exact retry/diagnostic message from the
    /// existing failure pairing; a stale failure naming a superseded attempt
    /// settles nothing.
    pub fn settle_approval_failure(
        &mut self,
        command: &artisan_domain::RespondApproval,
        failure: &ProtocolFailure,
        cx: &mut Context<Self>,
    ) -> Option<AnswerSettlement> {
        let gate = self
            .approval_gates
            .get_mut(command.approval_id().as_str())?;
        if gate.last_request_id() != Some(command.request_id()) {
            return None;
        }
        let settlement = gate.settle_failure(command.request_id(), failure);
        cx.notify();
        Some(settlement)
    }

    /// Settles one recorded question against its row gate.
    ///
    /// Identity rules match [`Self::settle_approval_answered`].
    pub fn settle_question_answered(
        &mut self,
        state: &EngineObservationState,
        command: &artisan_domain::RespondQuestion,
        receipt: &RespondQuestionReceipt,
        cx: &mut Context<Self>,
    ) -> Option<AnswerPairing> {
        let gate = self
            .question_gates
            .get_mut(command.question_id().as_str())?;
        if gate.last_request_id() != Some(command.request_id()) {
            return None;
        }
        let pairing = gate.settle_receipt(state, command, receipt);
        cx.notify();
        Some(pairing)
    }

    /// Settles one failed question against its row gate.
    ///
    /// The gate reopens with the exact retry/diagnostic message from the
    /// existing failure pairing while keeping the staged draft and selection
    /// for the explicit retry; identity rules match
    /// [`Self::settle_approval_failure`].
    pub fn settle_question_failure(
        &mut self,
        command: &artisan_domain::RespondQuestion,
        failure: &ProtocolFailure,
        cx: &mut Context<Self>,
    ) -> Option<AnswerSettlement> {
        let gate = self
            .question_gates
            .get_mut(command.question_id().as_str())?;
        if gate.last_request_id() != Some(command.request_id()) {
            return None;
        }
        let settlement = gate.settle_failure(command.request_id(), failure);
        cx.notify();
        Some(settlement)
    }

    /// Returns whether one approval row has an answer flight outstanding.
    #[must_use]
    pub fn approval_in_flight(&self, block_key: &str) -> bool {
        self.approval_gates
            .get(block_key)
            .is_some_and(ApprovalAnswerGate::is_in_flight)
    }

    /// Returns the current failure message for one approval row, if any.
    #[must_use]
    pub fn approval_failure_message(&self, block_key: &str) -> Option<&str> {
        self.approval_gates
            .get(block_key)
            .and_then(ApprovalAnswerGate::failure_message)
    }

    /// Returns whether one question row has an answer flight outstanding.
    #[must_use]
    pub fn question_in_flight(&self, block_key: &str) -> bool {
        self.question_gates
            .get(block_key)
            .is_some_and(QuestionAnswerGate::is_in_flight)
    }

    /// Returns the current failure message for one question row, if any.
    #[must_use]
    pub fn question_failure_message(&self, block_key: &str) -> Option<&str> {
        self.question_gates
            .get(block_key)
            .and_then(QuestionAnswerGate::failure_message)
    }
}
