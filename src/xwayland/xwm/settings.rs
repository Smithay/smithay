//! Types related to XSETTINGS handling
//!
//! See <https://specifications.freedesktop.org/xsettings-spec/0.5/>

use std::{borrow::Borrow, collections::HashMap, hash::Hash};

use tracing::debug;
use x11rb::{
    connection::Connection,
    errors::{ConnectionError, ReplyOrIdError},
    protocol::xproto::{ConnectionExt as _, PropMode, Window, WindowClass},
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};
// use std::{borrow::Borrow, hash::Hash};

#[derive(Debug)]
pub(super) struct XSettings {
    pub(super) window: Window,
    atoms: super::Atoms,
    values: HashMap<String, Setting>,
    current_serial: u32,
}

#[derive(Debug)]
struct Setting {
    val: Value,
    last_changed_serial: u32,
}

#[derive(Debug)]
/// XSETTINGS value
pub enum Value {
    /// String value
    String(String),
    /// Integer value
    Integer(i32),
    /// Color value
    Color([u16; 4]),
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}
impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::Integer(value)
    }
}
impl From<[u16; 4]> for Value {
    fn from(value: [u16; 4]) -> Self {
        Value::Color(value)
    }
}

const fn pad(len: usize) -> usize {
    (4 - (len % 4)) % 4
}

/// XSETTINGS Name validation error
#[derive(Debug, thiserror::Error)]
pub enum NameError {
    /// Name contained an invalid character. Only a-z, A-Z, 0-9, /, _ are allowed.
    #[error("Name contained an invalid character. Only a-z, A-Z, 0-9, /, _ are allowed.")]
    DisallowedCharacter,
    /// Name started or ended on a slash.
    #[error("Name started or ended on a slash.")]
    SlashAtStartOrEnd,
    /// Name contained two consecutive slashes.
    #[error("Name contained two consecutive slashes.")]
    DoubleSlash,
    /// Name contained a number at the start or immediately following a slash.
    #[error("Name contained a number at the start or immediately following a slash.")]
    LeadingNumber,
    /// Name was empty.
    #[error("Name was empty.")]
    EmptyName,
}

impl XSettings {
    pub(super) fn new(
        conn: &RustConnection,
        depth: u8,
        root: Window,
        atoms: &super::Atoms,
    ) -> Result<XSettings, ReplyOrIdError> {
        let win = conn.generate_id()?;
        conn.create_window(
            depth,
            win,
            root,
            // x, y, width, height, border width
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &Default::default(),
        )?;
        conn.set_selection_owner(win, atoms._XSETTINGS_S0, x11rb::CURRENT_TIME)?;
        debug!(window = win, "Created XSettings window");

        let mut this = XSettings {
            window: win,
            atoms: *atoms,
            values: HashMap::default(),
            current_serial: 0,
        };
        this.update(conn)?;
        Ok(this)
    }

    pub fn set(&mut self, name: impl Into<String>, value: impl Into<Value>) -> Result<(), NameError> {
        let name = name.into();
        if name
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '/' || c == '_'))
            .is_some()
        {
            return Err(NameError::DisallowedCharacter);
        }
        if name.is_empty() {
            return Err(NameError::EmptyName);
        }
        if name.starts_with('/') || name.ends_with('/') {
            return Err(NameError::SlashAtStartOrEnd);
        }
        if name.split('/').any(str::is_empty) {
            return Err(NameError::DoubleSlash);
        }
        if name
            .split('/')
            .any(|s| str::starts_with(s, |c: char| c.is_ascii_digit()))
        {
            return Err(NameError::LeadingNumber);
        }

        self.values.insert(
            name,
            Setting {
                val: value.into(),
                last_changed_serial: self.current_serial,
            },
        );

        Ok(())
    }

    #[allow(dead_code)]
    pub fn get<Q>(&self, name: &Q) -> Option<&Value>
    where
        Q: Hash + Eq + ?Sized,
        String: Borrow<Q>,
    {
        self.values.get(name).map(|setting| &setting.val)
    }

    #[allow(dead_code)]
    pub fn remove<Q>(&mut self, name: &Q) -> Option<Value>
    where
        Q: Hash + Eq + ?Sized,
        String: Borrow<Q>,
    {
        self.values.remove(name).map(|setting| setting.val)
    }

    fn serialize(&mut self) -> Vec<u8> {
        let mut data = Vec::new();

        if cfg!(target_endian = "little") {
            data.push(0);
        } else {
            data.push(1);
        }
        data.extend_from_slice(&[0u8; 3]);
        data.extend_from_slice(&self.current_serial.to_ne_bytes());
        data.extend_from_slice(&(self.values.len() as i32).to_ne_bytes());

        for (name, setting) in self.values.iter() {
            debug_assert!(name.is_ascii());
            let name = name.as_bytes();

            data.extend(&setting.val._type().to_ne_bytes());
            data.push(0u8);
            data.extend(&(name.len() as u16).to_ne_bytes());
            data.extend(name);
            data.extend(std::iter::repeat(0u8).take(pad(name.len())));
            data.extend(&setting.last_changed_serial.to_ne_bytes());

            setting.val.serialize(&mut data);
        }

        self.current_serial = self.current_serial.wrapping_add(1);

        data
    }

    pub(super) fn update(&mut self, conn: &RustConnection) -> Result<(), ConnectionError> {
        conn.change_property8(
            PropMode::REPLACE,
            self.window,
            self.atoms._XSETTINGS_SETTINGS,
            self.atoms._XSETTINGS_SETTINGS,
            &self.serialize(),
        )
        .map(|_| ())
    }
}

impl Value {
    fn _type(&self) -> i8 {
        match self {
            Value::Integer(_) => 0,
            Value::String(_) => 1,
            Value::Color(_) => 2,
        }
    }

    fn serialize(&self, data: &mut Vec<u8>) {
        match self {
            Value::Integer(val) => {
                data.extend(&val.to_ne_bytes());
            }
            Value::String(val) => {
                let val = val.as_bytes();
                data.extend(&(val.len() as u32).to_ne_bytes());
                data.extend(val);
                data.extend(std::iter::repeat(0u8).take(pad(val.len())));
            }
            Value::Color(val) => {
                for component in val {
                    data.extend(&component.to_ne_bytes());
                }
            }
        }
    }
}
