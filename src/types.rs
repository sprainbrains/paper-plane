use gtk::glib;

macro_rules! id_newtype {
    ($name:ident, $inner:ty) => {
        #[derive(
            Copy,
            Clone,
            Default,
            Eq,
            PartialEq,
            Ord,
            PartialOrd,
            Hash,
            Debug,
            gtk::glib::ValueDelegate,
        )]
        #[value_delegate(from = $inner)]
        #[repr(transparent)]
        pub(crate) struct $name($inner);

        impl $name {
            pub(crate) const fn new(value: $inner) -> Self {
                Self(value)
            }

            pub(crate) const fn get(self) -> $inner {
                self.0
            }
        }

        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                Self::new(value)
            }
        }

        impl From<$name> for $inner {
            fn from(value: $name) -> Self {
                value.get()
            }
        }

        impl From<&$name> for $inner {
            fn from(value: &$name) -> Self {
                value.get()
            }
        }
    };
}

// ClientId, UserId, ChatId, and MessageId remain aliases because they flow
// pervasively through GObject properties, GTK action values, and TDLib calls.
// Converting them safely requires a broader API-boundary migration than the
// isolated ID types below. MessageSenderId also deliberately shares TDLib's
// numeric namespace with both user and chat IDs.
pub(crate) type ClientId = i32;
pub(crate) type UserId = i64;
pub(crate) type ChatId = i64;
pub(crate) type MessageId = i64;
pub(crate) type MessageSenderId = i64;

id_newtype!(SecretChatId, i32);
id_newtype!(ChatFolderId, i32);

#[cfg(test)]
mod tests {
    use glib::prelude::StaticType;

    use super::*;

    #[test]
    fn wrapped_ids_keep_integer_glib_type_and_zero_default() {
        assert_eq!(SecretChatId::static_type(), i32::static_type());
        assert_eq!(ChatFolderId::static_type(), i32::static_type());
        assert_eq!(SecretChatId::default().get(), 0);
        assert_eq!(ChatFolderId::default().get(), 0);
    }
}
