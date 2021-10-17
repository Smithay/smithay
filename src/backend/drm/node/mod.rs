pub(crate) mod constants;

use constants::*;
use libc::dev_t;

use std::{
    fmt::{self, Display, Formatter},
    fs, io,
    os::unix::prelude::{AsRawFd, IntoRawFd, RawFd},
    path::PathBuf,
};

use nix::{
    fcntl::{self, OFlag},
    sys::stat::{fstat, major, minor, Mode},
    unistd::close,
};

/// A node which refers to a DRM device.
#[derive(Debug)]
pub struct DrmNode {
    // Always `Some`, None variant is used when taking ownership
    fd: Option<RawFd>,
    dev: dev_t,
    ty: NodeType,
}

impl DrmNode {
    /// Creates a DRM node from a file descriptor.
    ///
    /// This function takes ownership of the passed in file descriptor, which will be closed when
    /// dropped.
    pub fn from_fd<A: AsRawFd>(fd: A) -> Result<DrmNode, CreateDrmNodeError> {
        let fd = fd.as_raw_fd();
        let stat = fstat(fd).map_err(Into::<io::Error>::into)?;
        let dev = stat.st_rdev;
        let major = major(dev);
        let minor = minor(dev);

        if !is_device_drm(major, minor) {
            return Err(CreateDrmNodeError::NotDrmNode);
        }

        /*
        The type of the DRM node is determined by the minor number ranges.

        0-63 -> Primary
        64-127 -> Control
        128-255 -> Render
        */
        let ty = match minor >> 6 {
            0 => NodeType::Primary,
            1 => NodeType::Control,
            2 => NodeType::Render,
            _ => return Err(CreateDrmNodeError::NotDrmNode),
        };

        Ok(DrmNode {
            fd: Some(fd),
            dev,
            ty,
        })
    }

    /// Creates a DRM node of the specified type using the same DRM device as the provided node.
    /// The provided node will be consumed if the new node is successfully created.
    ///
    /// This function is useful for obtaining the render node of a DRM device when the provided
    /// node is a primary node.
    pub fn from_node_with_type(node: DrmNode, ty: NodeType) -> Result<DrmNode, ConvertNodeError> {
        from_node_with_type(node, ty)
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
        fs::read_link(format!("/proc/self/fd/{:?}", self.as_raw_fd())).ok()
    }

    /// Returns the major device number of the DRM device.
    pub fn major(&self) -> u64 {
        major(self.dev_id())
    }

    /// Returns the minor device number of the DRM device.
    pub fn minor(&self) -> u64 {
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

impl IntoRawFd for DrmNode {
    fn into_raw_fd(mut self) -> RawFd {
        self.fd.take().unwrap()
    }
}

impl AsRawFd for DrmNode {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.unwrap()
    }
}

impl Drop for DrmNode {
    fn drop(&mut self) {
        if let Some(fd) = self.fd {
            let _ = close(fd);
        }
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

/// A DRM node could not be used to get an alternative type of node.
///
/// If this error is returned, the original DRM node may be retrieved using [`ConvertNodeError::node`].
#[derive(Debug, thiserror::Error)]
#[error("{kind}")]
pub struct ConvertNodeError {
    node: DrmNode,
    kind: ConvertErrorKind,
}

impl ConvertNodeError {
    /// Returns the original DrmNode.
    pub fn node(self) -> DrmNode {
        self.node
    }

    /// Returns the kind of error that occurred when obtaining a different type of DRM node.
    pub fn kind(&self) -> &ConvertErrorKind {
        &self.kind
    }
}

/// An error that may occur when obtaining a different type of DRM node.
#[derive(Debug, thiserror::Error)]
pub enum ConvertErrorKind {
    /// The requested node type is not available.
    #[error("the requested node type, {0}, is not available.")]
    NodeTypeNotAvailable(NodeType),

    /// An IO error occurred when when obtaining a different type of DRM node.
    #[error("{0}")]
    Io(io::Error),
}

impl From<io::Error> for ConvertErrorKind {
    fn from(err: io::Error) -> Self {
        ConvertErrorKind::Io(err)
    }
}

#[cfg(target_os = "linux")]
fn is_device_drm(major: u64, minor: u64) -> bool {
    use nix::sys::stat::stat;

    let path = format!("/sys/dev/char/{}:{}/device/drm", major, minor);
    stat(path.as_str()).is_ok()
}

#[cfg(target_os = "freebsd")]
pub fn is_device_drm(major: u64, _minor: u64) -> bool {
    use nix::sys::stat::makedev;
    use nix::sys::stat::SFlag;
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_int};

    let dev_name = Vec::<c_char>::with_capacity(255); // Matching value of SPECNAMELEN in FreeBSD 13+

    let buf: *mut c_char = unsafe {
        libc::devname_r(
            makedev(major, minor),
            SFlag::S_IFCHR.bits(), // Must be S_IFCHR or S_IFBLK
            dev_name.as_mut_ptr(),
            dev_name.len() as c_int,
        )
    };

    // Buffer was too small (weird issue with the size of buffer) or the device could not be named.
    if buf.is_null() {
        return Err(CreateDrmNodeError::NotDrmNode);
    }

    // SAFETY: The buffer written to by devname_r is guaranteed to be NUL terminated.
    let dev_name = unsafe { CStr::from_ptr(buf) };
    let dev_name = dev_name.to_str().expect("Returned device name is not valid utf8");

    dev_name.starts_with("drm/")
        || dev_name.starts_with("dri/card")
        || dev_name.starts_with("dri/control")
        || dev_name.starts_with("dri/renderD")
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn is_device_drm(major: u64, _minor: u64) -> bool {
    major == DRM_MAJOR
}

#[cfg(target_os = "linux")]
fn from_node_with_type(node: DrmNode, ty: NodeType) -> Result<DrmNode, ConvertNodeError> {
    match node_path(&node, ty) {
        Ok(path) => {
            let fd = match fcntl::open(&path, OFlag::O_RDWR | OFlag::O_CLOEXEC, Mode::empty())
                .map_err(Into::<io::Error>::into)
                .map_err(Into::<ConvertErrorKind>::into)
            {
                Ok(fd) => fd,
                Err(kind) => return Err(ConvertNodeError { node, kind }),
            };

            let result = DrmNode::from_fd(fd);

            match result {
                Ok(node) => Ok(node),

                // Some error occurred while opening the new node
                Err(CreateDrmNodeError::Io(err)) => Err(ConvertNodeError {
                    node,
                    kind: err.into(),
                }),

                _ => unreachable!(),
            }
        }

        Err(err) => Err(ConvertNodeError {
            node,
            kind: err.into(),
        }),
    }
}

#[cfg(target_os = "freebsd")]
fn from_node_with_type(node: DrmNode, ty: NodeType) -> Result<DrmNode, ConvertNodeError> {
    // TODO: Not implemented yet, so fail conversion
    Err(ConvertNodeError {
        node,
        kind: ConvertErrorKind::NodeTypeNotAvailable(ty),
    })
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn from_node_with_type(node: DrmNode, ty: NodeType) -> Result<DrmNode, ConvertNodeError> {
    // TODO: Not implemented yet, so fail conversion
    Err(ConvertNodeError {
        node,
        kind: ConvertErrorKind::NodeTypeNotAvailable(ty),
    })
}

/// Returns the path of a specific type of node from the same DRM device as an existing DrmNode.
#[cfg(target_os = "linux")]
fn node_path(node: &DrmNode, ty: NodeType) -> io::Result<PathBuf> {
    use std::io::ErrorKind;

    let major = node.major();
    let minor = node.minor();

    let read = fs::read_dir(format!("/sys/dev/char/{}:{}/device/drm", major, minor))?;

    for entry in read.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();

        // Only 1 primary, control and render node may exist simultaneously, so the
        // first occurrence is good enough.
        if name.starts_with(ty.minor_name_prefix()) {
            let mut path = entry.path();
            path.push(DIR_NAME);

            if path.exists() {
                return Ok(path);
            }
        }
    }

    Err(io::Error::new(
        ErrorKind::NotFound,
        format!(
            "Could not find node of type {} from DRM device {}:{}",
            ty, major, minor
        ),
    ))
}
