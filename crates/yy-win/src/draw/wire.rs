use super::*;
impl App {
    pub(super) fn refresh_wire_constraint(&mut self) {
        if let Some(Drag::Connect { draft, .. }) = &mut self.drag {
            draft.update(draft.pointer, key(VK_SHIFT));
            self.invalidate_overlay();
        }
    }
    pub(super) fn wire_key(&mut self, k: u16) -> bool {
        let Some(Drag::Connect { draft, .. }) = &mut self.drag else {
            return false;
        };
        match k {
            k if k == VK_SHIFT.0 => draft.update(draft.pointer, true),
            k if k == VK_TAB.0 => {
                draft.bend();
            }
            k if k == VK_BACK.0 => draft.undo_bend(),
            _ => return false,
        }
        self.invalidate_overlay();
        true
    }
}
