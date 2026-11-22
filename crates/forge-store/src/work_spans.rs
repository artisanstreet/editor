use crate::error::AtStep as _;
use crate::error::StorageError;
use artisan_api::{Item, ItemId, ItemKind, RunId, Seq, ThreadId, WorkSpan};
use rusqlite::{Connection, OptionalExtension, Row, params};

type Result<T, E = StorageError> = std::result::Result<T, E>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    Outside,
    Breaker,
    Member { run: Option<RunId>, worked: bool },
}

impl Place {
    pub(crate) fn of(item: &Item) -> Self {
        match (&item.parent_id, item.folds_into_work()) {
            (Some(_), _) => Self::Outside,
            (None, false) => Self::Breaker,
            (None, true) => Self::Member {
                run: item.run_id.clone(),
                worked: matches!(
                    item.kind,
                    ItemKind::Tool { .. } | ItemKind::Agent { .. } | ItemKind::Plan { .. }
                ),
            },
        }
    }

    fn worked(&self) -> u32 {
        u32::from(matches!(self, Self::Member { worked: true, .. }))
    }
}

fn worked_kind(kind_type: Option<&str>) -> bool {
    matches!(kind_type, Some("tool" | "agent" | "plan"))
}

enum Neighbour {
    Edge,
    Breaker,
    Member { position: Seq, run: Option<RunId> },
}

impl Neighbour {
    fn member_of(&self, run: Option<&RunId>) -> Option<Seq> {
        match self {
            Self::Member {
                position,
                run: theirs,
            } if theirs.as_ref() == run => Some(*position),
            Self::Member { .. } | Self::Edge | Self::Breaker => None,
        }
    }

    fn position(&self) -> Option<Seq> {
        match self {
            Self::Member { position, .. } => Some(*position),
            Self::Edge | Self::Breaker => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Span {
    first: Seq,
    last: Seq,
    run: Option<RunId>,
    items: u32,
    worked: u32,
}

impl Span {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            first: row.get(0)?,
            last: row.get(1)?,
            run: row.get::<_, Option<String>>(2)?.map(RunId::from),
            items: row.get(3)?,
            worked: row.get(4)?,
        })
    }

    fn shown(self) -> WorkSpan {
        WorkSpan {
            run_id: self.run,
            first: self.first,
            last: self.last,
            count: self.items,
            worked: self.worked > 0,
        }
    }
}

pub(crate) fn stored(conn: &Connection, item: &ItemId) -> Result<Place> {
    let row = conn
        .prepare_cached(include_str!("sql/work_spans/place.sql"))
        .at("work_spans/place.sql")?
        .query_row([item.as_str()], |row| {
            Ok((
                row.get::<_, bool>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .optional()
        .at("work_spans/place.sql")?;
    Ok(match row {
        None | Some((false, _, _, _)) => Place::Outside,
        Some((true, false, _, _)) => Place::Breaker,
        Some((true, true, run, kind)) => Place::Member {
            run: run.map(RunId::from),
            worked: worked_kind(kind.as_deref()),
        },
    })
}

pub(crate) fn moved(
    conn: &Connection,
    thread: &ThreadId,
    position: Seq,
    old: &Place,
    new: &Place,
) -> Result<()> {
    if old == new {
        return Ok(());
    }
    leave(conn, thread, position, old)?;
    enter(conn, thread, position, new)
}

fn leave(conn: &Connection, thread: &ThreadId, position: Seq, old: &Place) -> Result<()> {
    match old {
        Place::Outside => Ok(()),
        Place::Breaker => join_across(conn, thread, position),
        Place::Member { .. } => {
            let span = reaching(conn, thread, position)?
                .filter(|span| span.last >= position)
                .expect("invariant: every work row lies in a stored work span");
            let mut kept = Span {
                items: span.items - 1,
                worked: span.worked - old.worked(),
                ..span.clone()
            };
            if kept.items == 0 {
                delete(conn, thread, span.first)?;
                return join_across(conn, thread, position);
            }
            if position == span.first {
                kept.first = neighbour(conn, thread, position, Side::After)?
                    .position()
                    .expect("invariant: a span's later rows follow its first");
            } else if position == span.last {
                kept.last = neighbour(conn, thread, position, Side::Before)?
                    .position()
                    .expect("invariant: a span's earlier rows precede its last");
            }
            replace(conn, thread, span.first, &kept)
        }
    }
}

fn join_across(conn: &Connection, thread: &ThreadId, position: Seq) -> Result<()> {
    let before = neighbour(conn, thread, position, Side::Before)?;
    let after = neighbour(conn, thread, position, Side::After)?;
    let (
        Neighbour::Member {
            position: left,
            run,
        },
        Neighbour::Member {
            position: right,
            run: theirs,
        },
    ) = (before, after)
    else {
        return Ok(());
    };
    if run != theirs {
        return Ok(());
    }
    let left_span = ending_at(conn, thread, left)?;
    let right_span = starting_at(conn, thread, right)?;
    delete(conn, thread, right_span.first)?;
    replace(
        conn,
        thread,
        left_span.first,
        &Span {
            first: left_span.first,
            last: right_span.last,
            run,
            items: left_span.items + right_span.items,
            worked: left_span.worked + right_span.worked,
        },
    )
}

fn enter(conn: &Connection, thread: &ThreadId, position: Seq, new: &Place) -> Result<()> {
    let Place::Member { run, .. } = new else {
        if matches!(new, Place::Breaker) {
            split_around(conn, thread, position)?;
        }
        return Ok(());
    };
    if let Some(span) = reaching(conn, thread, position)?.filter(|span| span.last > position) {
        if &span.run == run {
            return replace(
                conn,
                thread,
                span.first,
                &Span {
                    items: span.items + 1,
                    worked: span.worked + new.worked(),
                    ..span
                },
            );
        }
        split_around(conn, thread, position)?;
    }
    let left = neighbour(conn, thread, position, Side::Before)?
        .member_of(run.as_ref())
        .map(|left| ending_at(conn, thread, left))
        .transpose()?;
    let right = neighbour(conn, thread, position, Side::After)?
        .member_of(run.as_ref())
        .map(|right| starting_at(conn, thread, right))
        .transpose()?;
    let alone = Span {
        first: position,
        last: position,
        run: run.clone(),
        items: 1,
        worked: new.worked(),
    };
    match (left, right) {
        (Some(left), Some(right)) => {
            delete(conn, thread, right.first)?;
            replace(
                conn,
                thread,
                left.first,
                &Span {
                    last: right.last,
                    items: left.items + 1 + right.items,
                    worked: left.worked + alone.worked + right.worked,
                    ..left
                },
            )
        }
        (Some(left), None) => replace(
            conn,
            thread,
            left.first,
            &Span {
                last: position,
                items: left.items + 1,
                worked: left.worked + alone.worked,
                ..left
            },
        ),
        (None, Some(right)) => {
            delete(conn, thread, right.first)?;
            insert(
                conn,
                thread,
                &Span {
                    first: position,
                    items: right.items + 1,
                    worked: right.worked + alone.worked,
                    ..right
                },
            )
        }
        (None, None) => insert(conn, thread, &alone),
    }
}

fn split_around(conn: &Connection, thread: &ThreadId, position: Seq) -> Result<()> {
    let Some(span) = reaching(conn, thread, position)?.filter(|span| span.last > position) else {
        return Ok(());
    };
    let left = members(conn, thread, span.first, position - 1, span.run.clone())?
        .expect("invariant: a span split inside keeps its first row");
    let right = members(conn, thread, position + 1, span.last, span.run.clone())?
        .expect("invariant: a span split inside keeps its last row");
    delete(conn, thread, span.first)?;
    insert(conn, thread, &left)?;
    insert(conn, thread, &right)
}

#[derive(Clone, Copy)]
enum Side {
    Before,
    After,
}

fn neighbour(conn: &Connection, thread: &ThreadId, position: Seq, side: Side) -> Result<Neighbour> {
    let (step, sql) = match side {
        Side::Before => (
            "work_spans/before.sql",
            include_str!("sql/work_spans/before.sql"),
        ),
        Side::After => (
            "work_spans/after.sql",
            include_str!("sql/work_spans/after.sql"),
        ),
    };
    let row = conn
        .prepare_cached(sql)
        .at(step)?
        .query_row(params![thread.as_str(), position], |row| {
            Ok((
                row.get::<_, Seq>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .optional()
        .at(step)?;
    Ok(match row {
        None => Neighbour::Edge,
        Some((_, false, _)) => Neighbour::Breaker,
        Some((position, true, run)) => Neighbour::Member {
            position,
            run: run.map(RunId::from),
        },
    })
}

fn reaching(conn: &Connection, thread: &ThreadId, position: Seq) -> Result<Option<Span>> {
    conn.prepare_cached(include_str!("sql/work_spans/reaching.sql"))
        .at("work_spans/reaching.sql")?
        .query_row(params![thread.as_str(), position], Span::from_row)
        .optional()
        .at("work_spans/reaching.sql")
}

fn ending_at(conn: &Connection, thread: &ThreadId, last: Seq) -> Result<Span> {
    Ok(conn
        .prepare_cached(include_str!("sql/work_spans/ending_at.sql"))
        .at("work_spans/ending_at.sql")?
        .query_row(params![thread.as_str(), last], Span::from_row)
        .optional()
        .at("work_spans/ending_at.sql")?
        .expect("invariant: the last row of a run of work ends a stored span"))
}

fn starting_at(conn: &Connection, thread: &ThreadId, first: Seq) -> Result<Span> {
    Ok(conn
        .prepare_cached(include_str!("sql/work_spans/starting_at.sql"))
        .at("work_spans/starting_at.sql")?
        .query_row(params![thread.as_str(), first], Span::from_row)
        .optional()
        .at("work_spans/starting_at.sql")?
        .expect("invariant: the first row of a run of work starts a stored span"))
}

fn members(
    conn: &Connection,
    thread: &ThreadId,
    from: Seq,
    to: Seq,
    run: Option<RunId>,
) -> Result<Option<Span>> {
    let (items, worked, first, last) = conn
        .prepare_cached(include_str!("sql/work_spans/members.sql"))
        .at("work_spans/members.sql")?
        .query_row(params![thread.as_str(), from, to], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, Option<Seq>>(2)?,
                row.get::<_, Option<Seq>>(3)?,
            ))
        })
        .at("work_spans/members.sql")?;
    Ok(first.zip(last).map(|(first, last)| Span {
        first,
        last,
        run,
        items,
        worked,
    }))
}

fn insert(conn: &Connection, thread: &ThreadId, span: &Span) -> Result<()> {
    conn.prepare_cached(include_str!("sql/work_spans/insert.sql"))
        .at("work_spans/insert.sql")?
        .execute(params![
            thread.as_str(),
            span.first,
            span.last,
            span.run.as_ref().map(RunId::as_str),
            span.items,
            span.worked
        ])
        .at("work_spans/insert.sql")?;
    Ok(())
}

fn delete(conn: &Connection, thread: &ThreadId, first: Seq) -> Result<()> {
    conn.prepare_cached(include_str!("sql/work_spans/delete.sql"))
        .at("work_spans/delete.sql")?
        .execute(params![thread.as_str(), first])
        .at("work_spans/delete.sql")?;
    Ok(())
}

fn replace(conn: &Connection, thread: &ThreadId, first: Seq, span: &Span) -> Result<()> {
    delete(conn, thread, first)?;
    insert(conn, thread, span)
}

pub(crate) fn between(
    conn: &Connection,
    thread: &ThreadId,
    after: i64,
    before: i64,
) -> Result<Vec<WorkSpan>> {
    let spans: Vec<Span> = conn
        .prepare_cached(include_str!("sql/work_spans/between.sql"))
        .at("work_spans/between.sql")?
        .query_map(params![thread.as_str(), after, before], Span::from_row)
        .at("work_spans/between.sql")?
        .collect::<rusqlite::Result<_>>()
        .at("work_spans/between.sql")?;
    let low = after.saturating_add(1).max(0).unsigned_abs();
    let high = before.saturating_sub(1).max(0).unsigned_abs();
    spans
        .into_iter()
        .map(|span| {
            if span.first >= low && span.last <= high {
                return Ok(Some(span));
            }
            members(
                conn,
                thread,
                span.first.max(low),
                span.last.min(high),
                span.run,
            )
        })
        .filter_map(Result::transpose)
        .map(|span| span.map(Span::shown))
        .collect()
}

pub(crate) fn backfill(conn: &Connection) -> Result<()> {
    let mut select = conn
        .prepare(include_str!("sql/migrations/root_items.sql"))
        .at("migrations/root_items.sql")?;
    let mut rows = select.query([]).at("migrations/root_items.sql")?;
    let mut open: Option<(ThreadId, Span)> = None;
    while let Some(row) = rows.next().at("migrations/root_items.sql")? {
        let thread = ThreadId::from(row.get::<_, String>(0).at("migrations/root_items.sql")?);
        let position: Seq = row.get(1).at("migrations/root_items.sql")?;
        let work: bool = row.get(2).at("migrations/root_items.sql")?;
        let run = row
            .get::<_, Option<String>>(3)
            .at("migrations/root_items.sql")?
            .map(RunId::from);
        let worked = u32::from(worked_kind(
            row.get::<_, Option<String>>(4)
                .at("migrations/root_items.sql")?
                .as_deref(),
        ));
        match &mut open {
            Some((open_thread, span)) if work && *open_thread == thread && span.run == run => {
                span.last = position;
                span.items += 1;
                span.worked += worked;
                continue;
            }
            Some(_) | None => {}
        }
        if let Some((finished_thread, span)) = open.take() {
            insert(conn, &finished_thread, &span)?;
        }
        if work {
            open = Some((
                thread,
                Span {
                    first: position,
                    last: position,
                    run,
                    items: 1,
                    worked,
                },
            ));
        }
    }
    if let Some((thread, span)) = open {
        insert(conn, &thread, &span)?;
    }
    Ok(())
}
