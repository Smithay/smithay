use super::{MissingExtensionError, X11Error};

/// The extension macro.
///
/// This macro generates a struct which checks for the presence of some X11 extensions and stores
/// the version supplied by the X server.
///
/// ```rust
/// extensions! {
///     // The extension to check for. This should correspond to the name of the extension inside x11rb's `x11rb::protocol::xproto::<name>` module path.
///     xfixes {
///         // The function used to query the available version of the extension. This will be inside the module path as explained above
///         xfixes_query_version,
///         // The minimum version of the extension that will be accepted.
///         minimum: (4, 0),
///         // The version of the extension to request.
///         request: (4, 0),
///     },
/// }
///
/// // The extensions may be checked then using the generated `Extensions` struct using the `check_extensions` function.
/// ```
macro_rules! extensions {
    (
        $(
            $extension:ident { // Extension name for path lookup
                $extension_fn:ident, // Function used to look up the version of the extension
                minimum: ($min_major:expr, $min_minor:expr),
                request: ($req_major:expr, $req_minor:expr),
            },
        )*
    ) => {
        #[derive(Debug, Copy, Clone)]
        pub struct Extensions {
            $(
                #[doc = concat!(" The version of the `", stringify!($extension), "` extension.")]
                pub $extension: (u32, u32),
            )*
        }

        impl Extensions {
            pub fn check_extensions<C: x11rb::connection::Connection>(connection: &C, logger: &slog::Logger) -> Result<Extensions, X11Error> {
                $(
                    let $extension = {
                        use x11rb::protocol::$extension::{ConnectionExt as _, X11_EXTENSION_NAME};

                        if connection.extension_information(X11_EXTENSION_NAME)?.is_some() {
                            let version = connection.$extension_fn($req_major, $req_minor)?.reply()?;

                            #[allow(unused_comparisons)] // Macro comparisons
                            if version.major_version >= $req_major
                                || (version.major_version == $req_major && version.minor_version >= $req_minor)
                            {
                                slog::info!(
                                    logger,
                                    "Loaded extension {} version {}.{}",
                                    X11_EXTENSION_NAME,
                                    version.major_version,
                                    version.minor_version,
                                );

                                (version.major_version, version.minor_version)
                            } else {
                                slog::error!(
                                    logger,
                                    "{} extension version is too low (have {}.{}, expected {}.{})",
                                    X11_EXTENSION_NAME,
                                    version.major_version,
                                    version.minor_version,
                                    $req_major,
                                    $req_minor,
                                );

                                return Err(MissingExtensionError::WrongVersion {
                                    name: X11_EXTENSION_NAME,
                                    required_major: $req_major,
                                    required_minor: $req_minor,
                                    available_major: version.major_version,
                                    available_minor: version.minor_version,
                                }.into());
                            }
                        } else {
                            slog::error!(logger, "{} extension not found", X11_EXTENSION_NAME);

                            return Err(MissingExtensionError::NotFound {
                                name: X11_EXTENSION_NAME,
                                major: $min_major,
                                minor: $min_minor,
                            }
                            .into());
                        }
                    };
                )*

                Ok(Extensions {
                    $(
                        $extension,
                    )*
                })
            }
        }
    };
}

extensions! {
    present {
        present_query_version,
        minimum: (1, 0),
        request: (1, 0),
    },

    xfixes {
        xfixes_query_version,
        minimum: (4, 0),
        request: (4, 0),
    },

    dri3 {
        dri3_query_version,
        minimum: (1, 0),
        request: (1, 2),
    },
}
