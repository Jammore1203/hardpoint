//! Small compile-time helpers. Declared with `#[macro_use]` before every other
//! module so the macros are visible crate-wide.

/// A minimal bitflags implementation.
///
/// The real `bitflags` crate would be one more dependency for perhaps eighty
/// lines of generated code, and we only need containment tests and the bitwise
/// operators.
#[macro_export]
macro_rules! bitflags_lite {
    (
        $(#[$outer:meta])*
        pub struct $name:ident : $ty:ty {
            $(
                $(#[$inner:meta])*
                const $flag:ident = $value:expr;
            )*
        }
    ) => {
        $(#[$outer])*
        #[derive(Copy, Clone, PartialEq, Eq, Hash, Default)]
        pub struct $name(pub $ty);

        impl $name {
            pub const NONE_FLAGS: $name = $name(0);
            $(
                $(#[$inner])*
                pub const $flag: $name = $name($value);
            )*

            #[inline(always)]
            pub const fn empty() -> $name { $name(0) }
            #[inline(always)]
            pub const fn bits(self) -> $ty { self.0 }
            #[inline(always)]
            pub const fn from_bits_truncate(b: $ty) -> $name { $name(b) }
            #[inline(always)]
            pub const fn contains(self, other: $name) -> bool { (self.0 & other.0) == other.0 }
            #[inline(always)]
            pub const fn intersects(self, other: $name) -> bool { (self.0 & other.0) != 0 }
            #[inline(always)]
            pub const fn is_empty(self) -> bool { self.0 == 0 }
            #[inline(always)]
            pub fn insert(&mut self, other: $name) { self.0 |= other.0; }
            #[inline(always)]
            pub fn remove(&mut self, other: $name) { self.0 &= !other.0; }
            #[inline(always)]
            pub fn set(&mut self, other: $name, on: bool) {
                if on { self.insert(other) } else { self.remove(other) }
            }
        }

        impl std::ops::BitOr for $name {
            type Output = $name;
            #[inline(always)]
            fn bitor(self, rhs: $name) -> $name { $name(self.0 | rhs.0) }
        }
        impl std::ops::BitAnd for $name {
            type Output = $name;
            #[inline(always)]
            fn bitand(self, rhs: $name) -> $name { $name(self.0 & rhs.0) }
        }
        impl std::ops::BitOrAssign for $name {
            #[inline(always)]
            fn bitor_assign(&mut self, rhs: $name) { self.0 |= rhs.0; }
        }
        impl std::ops::Not for $name {
            type Output = $name;
            #[inline(always)]
            fn not(self) -> $name { $name(!self.0) }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({:#b})", stringify!($name), self.0)
            }
        }
    };
}
