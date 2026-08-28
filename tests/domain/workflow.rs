//! Workflow-shape coverage: listings, commands, receipts, and durable facts.

use artisan_domain::{
    AttachProject, Command, CommandReceipt, CreateThread, DIRECTORY_LISTING_MAX_ENTRIES,
    DIRECTORY_LISTING_MAX_PLACES, DirectoryEntry, DirectoryId, DirectoryKind, DirectoryListing,
    DirectoryListingError, DirectoryPlace, DisplayName, Event, FirstMessageQueued, ListDirectories,
    ListProjectThreads, MessageBody, MessageBodyError, MessageId, PlaceKind, ProjectAttached,
    ProjectId, ProjectSummary, Query, QueueFirstMessage, QueuedMessage, ReceiptDisposition,
    RequestId, RootPath, THREAD_LISTING_MAX_THREADS, ThreadCreated, ThreadId, ThreadListing,
    ThreadListingError, ThreadSummary, ThreadTitle, UnixMillis,
};

fn entry_at(index: usize) -> DirectoryEntry {
    DirectoryEntry {
        directory_id: DirectoryId::parse(format!("dir-{index}")).expect("fixture ids are valid"),
        display_name: DisplayName::parse(format!("Folder {index}")).expect("fixture names valid"),
        kind: if index == 0 {
            DirectoryKind::Root
        } else {
            DirectoryKind::Directory
        },
        has_children: index.is_multiple_of(2),
    }
}

fn place_at(index: usize) -> DirectoryPlace {
    let kinds = [
        PlaceKind::Home,
        PlaceKind::Desktop,
        PlaceKind::Documents,
        PlaceKind::Downloads,
        PlaceKind::Music,
        PlaceKind::Pictures,
        PlaceKind::Videos,
    ];
    DirectoryPlace {
        directory_id: DirectoryId::parse(format!("place-{index}")).expect("fixture ids are valid"),
        display_name: DisplayName::parse(format!("Place {index}")).expect("fixture names valid"),
        kind: kinds[index % kinds.len()],
    }
}

fn thread_summary(project: ProjectId, index: usize) -> ThreadSummary {
    let offset = i64::try_from(index).expect("fixture indices fit i64");
    ThreadSummary {
        thread_id: ThreadId::parse(format!("th-{index}")).expect("fixture ids are valid"),
        project_id: project,
        title: ThreadTitle::parse(format!("Thread {index}")).expect("fixture titles are valid"),
        created_at: UnixMillis::from_millis(1_000 + offset),
        updated_at: UnixMillis::from_millis(2_000 + offset),
    }
}

#[test]
fn directory_listing_enforces_both_collection_bounds() {
    let places = (0..DIRECTORY_LISTING_MAX_PLACES).map(place_at).collect();
    let entries = (0..DIRECTORY_LISTING_MAX_ENTRIES).map(entry_at).collect();
    let parent = DirectoryId::parse("dir-parent").expect("the fixture is valid");

    let listing = DirectoryListing::new(places, entries, Some(parent.clone()))
        .expect("bounds-sized collections fit");

    assert_eq!(listing.places().len(), DIRECTORY_LISTING_MAX_PLACES);
    assert_eq!(listing.entries().len(), DIRECTORY_LISTING_MAX_ENTRIES);
    assert_eq!(listing.parent(), Some(&parent));

    // One place past the documented ceiling is refused with counts attached.
    let too_many_places = (0..=DIRECTORY_LISTING_MAX_PLACES).map(place_at).collect();
    assert_eq!(
        DirectoryListing::new(too_many_places, Vec::new(), None),
        Err(DirectoryListingError::TooManyPlaces {
            count: DIRECTORY_LISTING_MAX_PLACES + 1,
            maximum: DIRECTORY_LISTING_MAX_PLACES,
        })
    );

    // One entry past the documented ceiling is likewise refused.
    let too_many_entries = (0..=DIRECTORY_LISTING_MAX_ENTRIES).map(entry_at).collect();
    assert_eq!(
        DirectoryListing::new(Vec::new(), too_many_entries, None),
        Err(DirectoryListingError::TooManyEntries {
            count: DIRECTORY_LISTING_MAX_ENTRIES + 1,
            maximum: DIRECTORY_LISTING_MAX_ENTRIES,
        })
    );
}

#[test]
fn thread_listing_enforces_its_deliberate_bound() {
    let project = ProjectId::parse("proj-1").expect("the fixture is valid");
    let threads = (0..THREAD_LISTING_MAX_THREADS)
        .map(|index| thread_summary(project.clone(), index))
        .collect();

    let listing = ThreadListing::new(threads).expect("a bounds-sized listing fits");
    assert_eq!(listing.threads().len(), THREAD_LISTING_MAX_THREADS);

    let over_bound = (0..=THREAD_LISTING_MAX_THREADS)
        .map(|index| thread_summary(project.clone(), index))
        .collect();
    assert_eq!(
        ThreadListing::new(over_bound),
        Err(ThreadListingError::TooManyThreads {
            count: THREAD_LISTING_MAX_THREADS + 1,
            maximum: THREAD_LISTING_MAX_THREADS,
        })
    );
}

#[test]
fn commands_carry_request_ids_and_never_forge_minted_inputs() {
    let attach_request = RequestId::parse("req-attach-1").expect("the fixture is valid");
    let attach_command = Command::AttachProject(AttachProject {
        request_id: attach_request.clone(),
        // Only the opaque directory crosses the boundary; the project id is
        // absent because Forge mints it during acceptance.
        directory_id: DirectoryId::parse("dir-9").expect("the fixture is valid"),
    });
    assert_eq!(attach_command.request_id(), &attach_request);

    let create_request = RequestId::parse("req-create-1").expect("the fixture is valid");
    let project = ProjectId::parse("proj-7").expect("the fixture is valid");
    let create_command = Command::CreateThread(CreateThread {
        request_id: create_request.clone(),
        project_id: project.clone(),
        // No thread id exists as an input: creation names only the owner.
        title: ThreadTitle::parse("New thread").expect("the fixture is valid"),
    });
    assert_eq!(create_command.request_id(), &create_request);

    let queue_request = RequestId::parse("req-queue-1").expect("the fixture is valid");
    let thread = ThreadId::parse("th-7").expect("the fixture is valid");
    let queue_command = Command::QueueFirstMessage(QueueFirstMessage {
        request_id: queue_request.clone(),
        // References existing state; no message id exists as an input.
        thread_id: thread.clone(),
        body: MessageBody::parse("Please summarize the failing test.").expect("valid body"),
    });
    assert_eq!(queue_command.request_id(), &queue_request);

    // Every mutation carries its correlation identity.
    assert_ne!(attach_request, create_request);
    assert_ne!(create_request, queue_request);
}

#[test]
fn invalid_first_messages_never_reach_a_command() {
    // The command struct cannot be built from a blank submission because the
    // body newtype refuses it before any durable concept is involved.
    assert_eq!(MessageBody::parse("   "), Err(MessageBodyError::Blank));
}

#[test]
fn events_record_only_post_acceptance_facts() {
    let project_id = ProjectId::parse("proj-2").expect("the fixture is valid");
    let project = ProjectSummary {
        project_id: project_id.clone(),
        display_name: DisplayName::parse("artisan-editor").expect("the fixture is valid"),
        root_path: RootPath::parse(r"C:\dev\artisan-editor").expect("the fixture is valid"),
        attached_at: UnixMillis::from_millis(1_758_816_000_000),
    };

    let attached = Event::ProjectAttached(ProjectAttached {
        project: project.clone(),
    });
    assert!(matches!(attached, Event::ProjectAttached(ref event) if event.project == project));

    let thread = ThreadSummary {
        thread_id: ThreadId::parse("th-2").expect("the fixture is valid"),
        project_id: project_id.clone(),
        title: ThreadTitle::parse("New thread").expect("the fixture is valid"),
        created_at: UnixMillis::from_millis(1_758_816_000_000),
        updated_at: UnixMillis::from_millis(1_758_816_000_001),
    };
    let created = Event::ThreadCreated(ThreadCreated {
        thread: thread.clone(),
    });
    assert!(matches!(created, Event::ThreadCreated(ref event) if event.thread == thread));

    let queued_message = QueuedMessage {
        message_id: MessageId::parse("msg-2").expect("the fixture is valid"),
        thread_id: thread.thread_id.clone(),
        request_id: RequestId::parse("req-queue-2").expect("the fixture is valid"),
        body: MessageBody::parse("Start here.").expect("the fixture is valid"),
    };
    let queued = Event::FirstMessageQueued(FirstMessageQueued {
        message: queued_message.clone(),
    });
    assert!(
        matches!(queued, Event::FirstMessageQueued(ref event) if event.message == queued_message),
        "queued is an explicit durable state carrying the minted message id"
    );
}

#[test]
fn receipts_distinguish_accepted_from_duplicate() {
    let replayed = RequestId::parse("req-replay-1").expect("the fixture is valid");
    let first = CommandReceipt {
        request_id: replayed.clone(),
        disposition: ReceiptDisposition::Accepted,
    };
    let replay = CommandReceipt {
        request_id: replayed,
        disposition: ReceiptDisposition::Duplicate,
    };

    assert_eq!(first.disposition, ReceiptDisposition::Accepted);
    assert_eq!(replay.disposition, ReceiptDisposition::Duplicate);
    assert_ne!(
        first, replay,
        "identical replays answer differently only in disposition"
    );

    let duplicate_of_duplicate = CommandReceipt {
        request_id: RequestId::parse("req-replay-1").expect("the fixture is valid"),
        disposition: ReceiptDisposition::Duplicate,
    };
    assert_eq!(replay, duplicate_of_duplicate);
}

#[test]
fn queries_cover_exactly_the_selected_reads() {
    let browse_root = Query::ListDirectories(ListDirectories { parent: None });
    assert!(matches!(browse_root, Query::ListDirectories(_)));

    let parent = DirectoryId::parse("dir-nested").expect("the fixture is valid");
    let browse_nested = Query::ListDirectories(ListDirectories {
        parent: Some(parent),
    });
    assert!(matches!(browse_nested, Query::ListDirectories(_)));

    let list_threads = Query::ListProjectThreads(ListProjectThreads {
        project_id: ProjectId::parse("proj-3").expect("the fixture is valid"),
    });
    assert!(matches!(list_threads, Query::ListProjectThreads(_)));
}
