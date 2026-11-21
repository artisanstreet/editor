use super::*;

#[test]
fn a_sign_in_handed_over_before_a_sign_out_is_not_kept() {
    let sign_outs = SignOuts::default();
    let handed_over = sign_outs.ticket();
    let removed = sign_outs.sign_out(|| "removed");
    assert_eq!(removed, "removed");
    assert_eq!(
        sign_outs.keep(handed_over, || "saved"),
        None,
        "a sign-in started before the sign-out was saved after it"
    );
    let after = sign_outs.ticket();
    assert_eq!(sign_outs.keep(after, || "saved"), Some("saved"));
}
