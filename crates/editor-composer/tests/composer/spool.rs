use super::*;
use crate::composer::draft_body::DraftBody;

fn kept(text: &str) -> KeptDraft {
    KeptDraft {
        body: DraftBody {
            text: text.to_owned(),
            ..DraftBody::default()
        },
        base: None,
        post: None,
    }
}

#[test]
fn drafts_whose_host_and_scope_join_alike_are_kept_apart() {
    let directory = tempfile::tempdir().unwrap();
    let spool = DraftSpool::new(directory.path());
    let first = HostId("forge-a".into());
    let second = HostId("forge".into());
    spool
        .keep(&first, "new", kept("on the first Forge"), &[])
        .unwrap();
    spool
        .keep(
            &second,
            "a-new",
            kept("on the second Forge"),
            &[std::sync::Arc::from(&b"image"[..])],
        )
        .unwrap();

    assert_eq!(
        spool.take(&first, "new").unwrap(),
        Some(SpooledDraft {
            draft: kept("on the first Forge"),
            images: Vec::new(),
        })
    );
    spool.forget(&first, "new").unwrap();
    assert_eq!(
        spool.take(&second, "a-new").unwrap(),
        Some(SpooledDraft {
            draft: kept("on the second Forge"),
            images: vec![b"image".to_vec()],
        }),
        "forgetting one draft leaves the other's text and images"
    );
}
