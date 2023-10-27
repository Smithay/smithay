//! Module for abstractions on drm device nodes

pub(crate) mod constants;

use constants::*;

use std::{
    fmt::{self, Display, Formatter},
    io,
    os::unix::io::AsFd,
    path::{Path, PathBuf},
};

use rustix::fs::{fstat, major, minor, stat, Dev as dev_t, Stat};

/// A node which refers to a DRM device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DrmNode {
    dev: dev_t,
    ty: NodeType,
}

impl DrmNode {
    /// Creates a DRM node from an open drm device.
    ///
    /// This function does not take ownership of the passed in file descriptor.
    pub fn from_file<A: AsFd>(file: A) -> Result<DrmNode, CreateDrmNodeError> {
        let stat = fstat(file).map_err(Into::<io::Error>::into)?;
        DrmNode::from_stat(stat)
    }

    /// Creates a DRM node from path.
    pub fn from_path<A: AsRef<Path>>(path: A) -> Result<DrmNode, CreateDrmNodeError> {
        let stat = stat(path.as_ref()).map_err(Into::<io::Error>::into)?;
        DrmNode::from_stat(stat)
    }

    /// Creates a DRM node from a file stat.
    pub fn from_stat(stat: Stat) -> Result<DrmNode, CreateDrmNodeError> {
        let dev = stat.st_rdev;
        DrmNode::from_dev_id(dev)
    }

    /// Creates a DRM node from a dev_t
    pub fn from_dev_id(dev: dev_t) -> Result<DrmNode, CreateDrmNodeError> {
        if !is_device_drm(dev) {
            return Err(CreateDrmNodeError::NotDrmNode);
        }

        /*
        The type of the DRM node is determined by the minor number ranges.

        0-63 -> Primary
        64-127 -> Control
        128-255 -> Render
        */
        let ty = match minor(dev) >> 6 {
            0 => NodeType::Primary,
            1 => NodeType::Control,
            2 => NodeType::Render,
            _ => return Err(CreateDrmNodeError::NotDrmNode),
        };

        Ok(DrmNode { dev, ty })
    }

    /// Returns the type of the DRM node.
    pub fn ty(&self) -> NodeType {
        self.ty
    }

    /// Returns the device_id of the underlying DRM node.
    pub fn dev_id(&self) -> dev_t {
        self.dev
    }

    /// Returns the path of the open device if possible.
    pub fn dev_path(&self) -> Option<PathBuf> {
        node_path(self, self.ty).ok()
    }

    /// Returns the path of the specified node type matching the device, if available.
    pub fn dev_path_with_type(&self, ty: NodeType) -> Option<PathBuf> {
        node_path(self, ty).ok()
    }

    /// Returns a new node of the specified node type matching the device, if available.
    pub fn node_with_type(&self, ty: NodeType) -> Option<Result<DrmNode, CreateDrmNodeError>> {
        self.dev_path_with_type(ty).map(DrmNode::from_path)
    }

    /// Returns the major device number of the DRM device.
    pub fn major(&self) -> u32 {
        major(self.dev_id())
    }

    /// Returns the minor device number of the DRM device.
    pub fn minor(&self) -> u32 {
        minor(self.dev_id())
    }

    /// Returns whether the DRM device has render nodes.
    pub fn has_render(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            node_path(self, NodeType::Render).is_ok()
        }

        // TODO: More robust checks on non-linux.
        #[cfg(target_os = "freebsd")]
        {
            false
        }

        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        {
            false
        }
    }
}

impl Display for DrmNode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.ty.minor_name_prefix(), minor(self.dev_id()))
    }
}

/// A type of node
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum NodeType {
    /// A primary node may be used to allocate buffers.
    ///
    /// If no other node is present, this may be used to post a buffer to an output with mode-setting.
    Primary,

    /// A control node may be used for mode-setting.
    ///
    /// This is almost never used since no DRM API for control nodes is available yet.
    Control,

    /// A render node may be used by a client to allocate buffers.
    ///
    /// Mode-setting is not possible with a render node.
    Render,
}

impl NodeType {
    /// Returns a string representing the prefix of a minor device's name.
    ///
    /// For example, on Linux with a primary node, the returned string would be `card`.
    pub fn minor_name_prefix(&self) -> &str {
        match self {
            NodeType::Primary => PRIMARY_NAME,
            NodeType::Control => CONTROL_NAME,
            NodeType::Render => RENDER_NAME,
        }
    }
}

impl Display for NodeType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                NodeType::Primary => "Primary",
                NodeType::Control => "Control",
                NodeType::Render => "Render",
            }
        )
    }
}

/// An error that may occur when creating a DrmNode from a file descriptor.
#[derive(Debug, thiserror::Error)]
pub enum CreateDrmNodeError {
    /// Some underlying IO error occured while trying to create a DRM node.
    #[error("{0}")]
    Io(io::Error),

    /// The provided file descriptor does not refer to a DRM node.
    #[error("the provided file descriptor does not refer to a DRM node.")]
    NotDrmNode,
}

impl From<io::Error> for CreateDrmNodeError {
    fn from(err: io::Error) -> Self {
        CreateDrmNodeError::Io(err)
    }
}

/// Returns if the given device by major:minor pair is a drm device
#[cfg(target_os = "linux")]
pub fn is_device_drm(dev: dev_t) -> bool {
    let path = format!("/sys/dev/char/{}:{}/device/drm", major(dev), minor(dev));
    stat(path.as_str()).is_ok()
}

#[cfg(target_os = "freebsd")]
fn devname(dev: dev_t) -> Option<String> {
    use std::os::raw::{c_char, c_int};

    // Matching value of SPECNAMELEN in FreeBSD 13+
    let mut dev_name = vec![0u8; 255];

    let buf: *mut c_char = unsafe {
        libc::devname_r(
            dev,
            libc::S_IFCHR, // Must be S_IFCHR or S_IFBLK
            dev_name.as_mut_ptr() as *mut c_char,
            dev_name.len() as c_int,
        )
    };

    // Buffer was too small (weird issue with the size of buffer) or the device could not be named.
    if buf.is_null() {
        return None;
    }

    // SAFETY: The buffer written to by devname_r is guaranteed to be NUL terminated.
    unsafe { dev_name.set_len(libc::strlen(buf)) };

    Some(String::from_utf8(dev_name).expect("Returned device name is not valid utf8"))
}

/// Returns if the given device by major:minor pair is a drm device
#[cfg(target_os = "freebsd")]
pub fn is_device_drm(dev: dev_t) -> bool {
    devname(dev).map_or(false, |dev_name| {
        dev_name.starts_with("drm/")
            || dev_name.starts_with("dri/card")
            || dev_name.starts_with("dri/control")
            || dev_name.starts_with("dri/renderD")
    })
}

/// Returns if the given device by major:minor pair is a drm device
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn is_device_drm(dev: dev_t) -> bool {
    major(dev) == DRM_MAJOR
}

/// Returns the path of a specific type of node from the same DRM device as another path of the same node.
pub fn path_to_type<P: AsRef<Path>>(path: P, ty: NodeType) -> io::Result<PathBuf> {
    let stat = stat(path.as_ref()).map_err(Into::<io::Error>::into)?;
    dev_path(stat.st_rdev, ty)
}

/// Returns the path of a specific type of node from the same DRM device as an existing DrmNode.
pub fn node_path(node: &DrmNode, ty: NodeType) -> io::Result<PathBuf> {
    dev_path(node.dev, ty)
}

/// Returns the path of a specific type of node from the DRM device described by major and minor device numbers.
#[cfg(target_os = "linux")]
pub fn dev_path(dev: dev_t, ty: NodeType) -> io::Result<PathBuf> {
    use std::fs;
    use std::io::ErrorKind;

    if !is_device_drm(dev) {
        return Err(io::Error::new(
            ErrorKind::NotFound,
            format!("{}:{} is no DRM device", major(dev), minor(dev)),
        ));
    }

    let read = fs::read_dir(format!("/sys/dev/char/{}:{}/device/drm", major(dev), minor(dev)))?;

    for entry in read.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();

        // Only 1 primary, control and render node may exist simultaneously, so the
        // first occurrence is good enough.
        if name.starts_with(ty.minor_name_prefix()) {
            let path = [r"/", "dev", "dri", &name].iter().collect::<PathBuf>();
            if path.exists() {
                return Ok(path);
            }
        }
    }

    Err(io::Error::new(
        ErrorKind::NotFound,
        format!(
            "Could not find node of type {} from DRM device {}:{}",
            ty,
            major(dev),
            minor(dev)
        ),
    ))
}

/// Returns the path of a specific type of node from the DRM device described by major and minor device numbers.
#[cfg(target_os = "freebsd")]
fn dev_path(dev: dev_t, ty: NodeType) -> io::Result<PathBuf> {
    // Based on libdrm `drmGetMinorNameForFD`. Should be updated if the code
    // there is replaced with anything more sensible...

    use std::io::ErrorKind;

    if !is_device_drm(dev) {
        return Err(io::Error::new(
            ErrorKind::NotFound,
            format!("{}:{} is no DRM device", major(dev), minor(dev)),
        ));
    }

    if let Some(dev_name) = devname(dev) {
        let suffix = dev_name.trim_start_matches(|c: char| !c.is_numeric());
        if let Ok(old_id) = suffix.parse::<u32>() {
            let old_ty = match old_id >> 6 {
                0 => NodeType::Primary,
                1 => NodeType::Control,
                2 => NodeType::Render,
                _ => {
                    return Err(io::Error::new(
                        ErrorKind::NotFound,
                        format!("{}:{} is no DRM device", major(dev), minor(dev)),
                    ));
                }
            };
            let id = old_id - get_minor_base(old_ty) + get_minor_base(ty);
            let path = PathBuf::from(format!("/dev/dri/{}{}", ty.minor_name_prefix(), id));
            if path.exists() {
                return Ok(path);
            }
        }
    }

    Err(io::Error::new(
        ErrorKind::NotFound,
        format!(
            "Could not find node of type {} from DRM device {}:{}",
            ty,
            major(dev),
            minor(dev)
        ),
    ))
}

#[cfg(target_os = "openbsd")]
fn dev_path(dev: dev_t, ty: NodeType) -> io::Result<PathBuf> {
    use std::io::ErrorKind;

    if !is_device_drm(dev) {
        return Err(io::Error::new(
            ErrorKind::NotFound,
            format!("{}:{} is no DRM device", major(dev), minor(dev)),
        ));
    }

    let old_id = minor(dev);
    let old_ty = match old_id >> 6 {
        0 => NodeType::Primary,
        1 => NodeType::Control,
        2 => NodeType::Render,
        _ => {
            return Err(io::Error::new(
                ErrorKind::NotFound,
                format!("{}:{} is no DRM device", major(dev), minor(dev)),
            ));
        }
    };
    let id = old_id - get_minor_base(old_ty) + get_minor_base(ty);
    let path = PathBuf::from(format!("/dev/dri/{}{}", ty.minor_name_prefix(), id));
    if path.exists() {
        return Ok(path);
    }

    Err(io::Error::new(
        ErrorKind::NotFound,
        format!(
            "Could not find node of type {} from DRM device {}:{}",
            ty,
            major(dev),
            minor(dev)
        ),
    ))
}

#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
fn get_minor_base(type_: NodeType) -> u32 {
    match type_ {
        NodeType::Primary => 0,
        NodeType::Control => 64,
        NodeType::Render => 128,
    }
}
