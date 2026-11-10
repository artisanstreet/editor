use std::{cell::RefCell, rc::Rc};

pub struct SignInForm {
    pub email: Rc<RefCell<editor_ui::text::editor::Editor>>,
    pub second_factor: Rc<RefCell<editor_ui::text::editor::Editor>>,
}

impl Default for SignInForm {
    fn default() -> Self {
        use editor_ui::text::editor::{Editor, Mode};
        Self {
            email: Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))),
            second_factor: Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))),
        }
    }
}

impl SignInForm {
    pub fn request_link(&self, core: &mut editor_core::Core) {
        let device_name = core.device_name.clone();
        core.account_mut()
            .request_link(self.email.borrow().text(), device_name);
    }

    pub fn finish_sign_in(&mut self, core: &mut editor_core::Core) {
        let factor = editor_core::account::SecondFactor::parse(self.second_factor.borrow().text());
        core.account_mut().finish_sign_in(factor);
        self.second_factor.borrow_mut().set_text("");
    }
}
