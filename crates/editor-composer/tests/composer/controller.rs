use super::markdown_link;

#[test]
fn file_links_escape_markup_in_the_name_and_keep_the_path_one_link() {
    assert_eq!(
        markdown_link("*notes*_[draft].md", "/work/*notes*_[draft].md"),
        r"[\*notes\*\_\[draft\].md](</work/*notes*_[draft].md>)"
    );
    assert_eq!(
        markdown_link("a>b\nc.rs", "/work/a>b\nc.rs"),
        r"[a\>b c.rs](</work/a\>b%0Ac.rs>)"
    );
    assert_eq!(
        markdown_link("main.rs", r"C:\src\main.rs"),
        r"[main.rs](<C:\src\main.rs>)"
    );
    assert_eq!(
        markdown_link("x.rs", r"C:\[x]\x.rs"),
        r"[x.rs](<C:\\[x]\x.rs>)"
    );
}
