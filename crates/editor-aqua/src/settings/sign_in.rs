pub fn second_factor(input: &str) -> (Option<String>, Option<String>) {
    let code = input.trim();
    match (
        code.is_empty(),
        code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()),
    ) {
        (true, _) => (None, None),
        (false, true) => (Some(code.to_owned()), None),
        (false, false) => (None, Some(code.to_owned())),
    }
}

use std::{cell::RefCell, rc::Rc};

pub struct SignInForm {
    pub email: Rc<RefCell<crate::ui::text::editor::Editor>>,
    pub second_factor: Rc<RefCell<crate::ui::text::editor::Editor>>,
}

impl Default for SignInForm {
    fn default() -> Self {
        use crate::ui::text::editor::{Editor, Mode};
        Self {
            email: Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))),
            second_factor: Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))),
        }
    }
}

impl SignInForm {
    pub fn request_link(&self, core: &mut crate::core::Core) {
        let device_name = core.device_name.clone();
        core.account_mut()
            .request_link(self.email.borrow().text(), device_name);
    }

    pub fn finish_sign_in(&mut self, core: &mut crate::core::Core) {
        let (totp, recovery) = second_factor(self.second_factor.borrow().text());
        core.account_mut().finish_sign_in(totp, recovery);
        self.second_factor.borrow_mut().set_text("");
    }
}
