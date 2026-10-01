//! Thread-level questionnaires over the append-only observation ledger.
//!
//! A questionnaire is every question row sharing one group identity: the
//! questions one provider request asked together. The ledger is the only
//! authority: a question is open until a resolved row for its question
//! identity follows it, whether the user answered it through its run or the
//! run closed it as it ended.
//!
//! [`Repository::read_thread_questionnaire`] reads a questionnaire's state
//! so an answer can be routed to the run that asked each question.

use artisan_domain::{
    Observation, ObservationId, QuestionObservation, QuestionState, RunId, ThreadId,
};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

use crate::entities::observation_ledger;

use super::run_observation::decode_observation_checkpoint;
use super::{Repository, RepositoryError, corrupt_data, database_error};

/// Ledger rows read per page while scanning a thread for one questionnaire.
const QUESTIONNAIRE_SCAN_PAGE: u64 = 512;

/// One question of a questionnaire as the ledger records it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerQuestion {
    /// The requested question row.
    pub question: QuestionObservation,
    /// The run that asked it, which owns its answer.
    pub run_id: RunId,
    /// Whether a resolved row already follows it.
    pub resolved: bool,
}

impl Repository {
    /// Reads one questionnaire on a thread in the order it was asked.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] when a ledger row fails to decode or a
    /// database query fails.
    pub async fn read_thread_questionnaire(
        &self,
        thread_id: &ThreadId,
        group_id: &ObservationId,
    ) -> Result<Vec<LedgerQuestion>, RepositoryError> {
        scan_questionnaire(&self.database, thread_id, group_id).await
    }
}

/// Scans a thread's ledger for one questionnaire, oldest first.
async fn scan_questionnaire<C: ConnectionTrait>(
    connection: &C,
    thread_id: &ThreadId,
    group_id: &ObservationId,
) -> Result<Vec<LedgerQuestion>, RepositoryError> {
    let mut questions: Vec<LedgerQuestion> = Vec::new();
    let mut resolved_ids: Vec<ObservationId> = Vec::new();
    let mut after = 0_i64;
    loop {
        let rows = observation_ledger::Entity::find()
            .filter(observation_ledger::Column::ThreadId.eq(thread_id.as_str()))
            .filter(observation_ledger::Column::DeliverySequence.gt(after))
            .order_by_asc(observation_ledger::Column::DeliverySequence)
            .limit(QUESTIONNAIRE_SCAN_PAGE)
            .all(connection)
            .await
            .map_err(|source| database_error("scan questionnaire", source))?;
        let Some(last) = rows.last() else {
            break;
        };
        after = last.delivery_sequence;
        for row in &rows {
            let decoded = decode_observation_checkpoint(
                row.observation_version,
                row.observation_bytes.as_slice(),
            )
            .map_err(|source| corrupt_data("observation_ledger", "observation_bytes", source))?;
            let [Observation::Question(question)] = decoded.observations().as_slice() else {
                continue;
            };
            match question.state() {
                QuestionState::Resolved => resolved_ids.push(question.question_id().clone()),
                QuestionState::Requested if question.group_id() == group_id => {
                    questions.push(LedgerQuestion {
                        question: question.clone(),
                        run_id: RunId::parse(row.run_id.clone())
                            .map_err(|error| corrupt_data("observation_ledger", "run_id", error))?,
                        resolved: false,
                    });
                }
                QuestionState::Requested => {}
            }
        }
        if u64::try_from(rows.len()).unwrap_or(u64::MAX) < QUESTIONNAIRE_SCAN_PAGE {
            break;
        }
    }
    for question in &mut questions {
        question.resolved = resolved_ids
            .iter()
            .any(|resolved| resolved == question.question.question_id());
    }
    Ok(questions)
}
