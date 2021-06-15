//! Tools for handling surface roles
//!
//! In the Wayland protocol, surfaces can have several different roles, which
//! define how they are to be used. The core protocol defines 3 of these roles:
//!
//! - `shell_surface`: This surface is to be considered as what is most often
//!   called a "window".
//! - `pointer_surface`: This surface represent the contents of a pointer icon
//!   and replaces the default pointer.
//! - `subsurface`: This surface is part of a subsurface tree, and as such has
//!   a parent surface.
//!
//! A surface can have only one role at any given time. To change he role of a
//! surface, the client must first remove the previous role before assigning the
//! new one. A surface without a role is not displayed at all.
//!
//! This module provides tools to manage roles of a surface in a composable way
//! allowing all handlers of smithay to manage surface roles while being aware
//! of the possible role conflicts.
//!
//! ## General mechanism
//!
//! First, all roles need to have an unique type, holding its metadata and identifying it
//! to the type-system. Even if your role does not hold any metadata, you still need its
//! unique type, using a unit-like struct rather than `()`.
//!
//! You then need a type for managing the roles of a surface. This type holds information
//! about what is the current role of a surface, and what is the metadata associated with
//! it.
//!
//! For convenience, you can use the `define_roles!` macro provided by Smithay to define this
//! type. You can call it like this:
//!
//! ```
//! # use smithay::define_roles;
//! // Metadata for a first role
//! #[derive(Default)]
//! pub struct MyRoleMetadata {
//! }
//!
//! // Metadata for a second role
//! #[derive(Default)]
//! pub struct MyRoleMetadata2 {
//! }
//!
//! define_roles!(Roles =>
//!     // You can put several roles like this
//!     // first identifier is the name of the variant for this
//!     // role in the generated enum, second is the token type
//!     // for this role
//!     [MyRoleName, MyRoleMetadata]
//!     [MyRoleName2, MyRoleMetadata2]
//!     /* ... */
//! );
//!
//! ```
//!
//! And this will expand to an enum like this:
//!
//! ```ignore
//! pub enum Roles {
//!     NoRole,
//!     // The subsurface role is always inserted, as it is required
//!     // by the CompositorHandler
//!     Subsurface(::smithay::compositor::SubsurfaceAttributes),
//!     // all your other roles come here
//!     MyRoleName(MyRoleMetadata),
//!     MyRoleName2(MyRoleMetadata2),
//!     /* ... */
//! }
//! ```
//!
//! as well as implement a few trait for it, allowing it to be used by
//! all smithay handlers:
//!
//! - The trait [`RoleType`](RoleType),
//!   which defines it as a type handling roles
//! - For each of your roles, the trait [`Role<Token>`](Role)
//!   (where `Token` is your token type), marking its ability to handle this given role.
//!
//! All handlers that handle a specific role will require you to provide
//! them with a [`CompositorToken<U, R, H>`](crate::wayland::compositor::CompositorToken)
//! where `R: Role<TheToken>`.
//!
//! See the documentation of these traits for their specific definition and
//! capabilities.

/// An error type signifying that the surface does not have expected role
///
/// Generated if you attempt a role operation on a surface that does
/// not have the role you asked for.
#[derive(Debug)]
pub struct WrongRole;

impl std::fmt::Display for WrongRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Wrong role for surface.")
    }
}

impl std::error::Error for WrongRole {}

/// An error type signifying that the surface already has a role and
/// cannot be assigned an other
///
/// Generated if you attempt a role operation on a surface that does
/// not have the role you asked for.
#[derive(Debug)]
pub struct AlreadyHasRole;

impl std::fmt::Display for AlreadyHasRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Surface already has a role.")
    }
}

impl std::error::Error for AlreadyHasRole {}

/// A trait representing a type that can manage surface roles
pub trait RoleType {
    /// Check if the associated surface has a role
    ///
    /// Only reports if the surface has any role or no role.
    /// To check for a role in particular, see [`Role::has`].
    fn has_role(&self) -> bool;
}

/// A trait representing the capability of a [`RoleType`] to handle a given role
///
/// This trait allows to interact with the different roles a [`RoleType`] can
/// handle.
///
/// This trait is meant to be used generically, for example, to retrieve the
/// data associated with a given role with token `TheRole`:
///
/// ```ignore
/// let data = <MyRoles as Role<RoleData>>::data(my_roles)
///                 .expect("The surface does not have this role.");
/// ```
///
/// The methods of this trait are mirrored on
/// [`CompositorToken`](crate::wayland::compositor::CompositorToken) for easy
/// access to the role data of the surfaces.
///
/// Note that if a role is automatically handled for you by a Handler provided
/// by smithay, you should not set or unset it manually on a surface. Doing so
/// would likely corrupt the internal state of these handlers, causing spurious
/// protocol errors and unreliable behaviour overall.
pub trait Role<R>: RoleType {
    /// Set the role for the associated surface with default associated data
    ///
    /// Fails if the surface already has a role
    fn set(&mut self) -> Result<(), AlreadyHasRole>
    where
        R: Default,
    {
        self.set_with(Default::default()).map_err(|_| AlreadyHasRole)
    }

    /// Set the role for the associated surface with given data
    ///
    /// Fails if the surface already has a role and returns the data
    fn set_with(&mut self, data: R) -> Result<(), R>;

    /// Check if the associated surface has this role
    fn has(&self) -> bool;

    /// Access the data associated with this role if its the current one
    fn data(&self) -> Result<&R, WrongRole>;

    /// Mutably access the data associated with this role if its the current one
    fn data_mut(&mut self) -> Result<&mut R, WrongRole>;

    /// Remove this role from the associated surface
    ///
    /// Fails if the surface does not currently have this role
    fn unset(&mut self) -> Result<R, WrongRole>;
}

/// The roles defining macro
///
/// See the docs of the [`wayland::compositor::roles`](wayland/compositor/roles/index.html) module
/// for an explanation of its use.
#[macro_export]
macro_rules! define_roles(
    ($enum_name: ident) => {
        define_roles!($enum_name =>);
    };
    ($enum_name:ident => $($(#[$role_attr:meta])* [$role_name: ident, $role_data: ty])*) => {
        define_roles!(__impl $enum_name =>
            // add in subsurface role
            [Subsurface, $crate::wayland::compositor::SubsurfaceRole]
            $($(#[$role_attr])* [$role_name, $role_data])*
        );
    };
    (__impl $enum_name:ident => $($(#[$role_attr:meta])* [$role_name: ident, $role_data: ty])*) => {
        pub enum $enum_name {
            NoRole,
            $($(#[$role_attr])* $role_name($role_data)),*
        }

        impl Default for $enum_name {
            fn default() -> $enum_name {
                $enum_name::NoRole
            }
        }

        impl $crate::wayland::compositor::roles::RoleType for $enum_name {
            fn has_role(&self) -> bool {
                if let $enum_name::NoRole = *self {
                    false
                } else {
                    true
                }
            }
        }

        $(
            $(#[$role_attr])*
            impl $crate::wayland::compositor::roles::Role<$role_data> for $enum_name {
                fn set_with(&mut self, data: $role_data) -> ::std::result::Result<(), $role_data> {
                    if let $enum_name::NoRole = *self {
                        *self = $enum_name::$role_name(data);
                        Ok(())
                    } else {
                        Err(data)
                    }
                }

                fn has(&self) -> bool {
                    if let $enum_name::$role_name(_) = *self {
                        true
                    } else {
                        false
                    }
                }

                fn data(&self) -> ::std::result::Result<
                                    &$role_data,
                                    $crate::wayland::compositor::roles::WrongRole
                                  >
                {
                    if let $enum_name::$role_name(ref data) = *self {
                        Ok(data)
                    } else {
                        Err($crate::wayland::compositor::roles::WrongRole)
                    }
                }

                fn data_mut(&mut self) -> ::std::result::Result<
                                            &mut $role_data,
                                            $crate::wayland::compositor::roles::WrongRole
                                          >
                {
                    if let $enum_name::$role_name(ref mut data) = *self {
                        Ok(data)
                    } else {
                        Err($crate::wayland::compositor::roles::WrongRole)
                    }
                }

                fn unset(&mut self) -> ::std::result::Result<
                                        $role_data,
                                        $crate::wayland::compositor::roles::WrongRole
                                       >
                {
                    // remove self to make borrow checker happy
                    let temp = ::std::mem::replace(self, $enum_name::NoRole);
                    if let $enum_name::$role_name(data) = temp {
                        Ok(data)
                    } else {
                        // put it back in place
                        *self = temp;
                        Err($crate::wayland::compositor::roles::WrongRole)
                    }
                }
            }
        )*
    };
);
