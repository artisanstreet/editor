use super::{host_kind, web_url};

#[test]
fn remotes_become_their_browser_pages() {
    let page = |remote| web_url(remote);
    assert_eq!(
        page("git@github.com:artisanstreet/editor.git").as_deref(),
        Some("https://github.com/artisanstreet/editor")
    );
    assert_eq!(
        page("https://github.com/sandersonstabo/shooter").as_deref(),
        Some("https://github.com/sandersonstabo/shooter")
    );
    assert_eq!(
        page("ssh://git@gitlab.com:2222/group/sub/repo.git").as_deref(),
        Some("https://gitlab.com/group/sub/repo")
    );
    assert_eq!(page("/srv/git/repo.git"), None);
    assert_eq!(page("C:\\repos\\thing"), None);
    assert_eq!(host_kind("https://github.com/o/r"), "github");
    assert_eq!(host_kind("https://git.example.com/o/r"), "other");
}
