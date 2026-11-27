use windows::Win32::UI::WindowsAndMessaging::*;

pub(super) const BUTTON: u16 = 0x80;
pub(super) const EDIT: u16 = 0x81;
pub(super) const LABEL: u16 = 0x82;
pub(super) const COMBO: u16 = 0x85;

// Standard DLGTEMPLATE and DLGITEMTEMPLATE records use UTF-16 strings, predefined
// control class ordinals and DWORD alignment before each item.
pub(super) struct Template(Vec<u16>);
impl Template {
    fn string(&mut self, value: &str) {
        self.0.extend(value.encode_utf16().chain(Some(0)));
    }
    fn dword(&mut self, n: u32) {
        self.0.extend([n as u16, (n >> 16) as u16]);
    }
    pub(super) fn item(&mut self, id: u16, class: u16, text: &str, rect: [u16; 4], style: u32) {
        self.item_with_class(id, [0xffff, class], text, rect, style);
    }
    pub(super) fn item_with_class(
        &mut self,
        id: u16,
        class: impl IntoIterator<Item = u16>,
        text: &str,
        rect: [u16; 4],
        style: u32,
    ) {
        if self.0.len() % 2 == 1 {
            self.0.push(0);
        }
        self.0[4] += 1;
        self.dword(WS_CHILD.0 | WS_VISIBLE.0 | style);
        self.dword(0);
        self.0.extend(rect);
        self.0.push(id);
        self.0.extend(class);
        self.string(text);
        self.0.push(0);
    }

    pub(super) fn new(title: &str, width: u16) -> Self {
        let mut t = Template(Vec::new());
        t.dword(
            WS_POPUP.0
                | WS_CAPTION.0
                | WS_SYSMENU.0
                | (DS_CENTER | DS_SETFONT | DS_MODALFRAME) as u32,
        );
        t.dword(0);
        t.0.extend([0, 0, 0, width, 0, 0, 0]);
        t.string(title);
        t.0.push(9);
        t.string("Segoe UI");
        t
    }
    pub(super) fn finish(mut self, height: u16) -> Vec<u32> {
        self.0[8] = height;
        self.0.push(0);
        self.0
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u32::from(p[0]) | (u32::from(p[1]) << 16))
            .collect()
    }
}
