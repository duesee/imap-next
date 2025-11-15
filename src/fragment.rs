use imap_codec::{
    fragmentizer::{FragmentInfo, Fragmentizer},
    imap_types::utils::escape_byte_string,
};
use tracing::{instrument, trace};

/// Describes the type of a fragment for logging purposes.
#[derive(Debug)]
pub enum FragmentKind {
    Line,
    Literal,
}

impl From<FragmentInfo> for FragmentKind {
    fn from(frament_info: FragmentInfo) -> Self {
        match frament_info {
            FragmentInfo::Line { .. } => Self::Line,
            FragmentInfo::Literal { .. } => Self::Literal,
        }
    }
}

#[instrument(name = "fragment", skip_all, fields(action = "read"))]
pub fn read_fragment(fragmentizer: &mut Fragmentizer) -> Option<FragmentInfo> {
    let fragment_info = fragmentizer.progress()?;

    trace!(
        type = ?FragmentKind::from(fragment_info),
        data = escape_byte_string(fragmentizer.fragment_bytes(fragment_info)),
        complete = fragmentizer.is_message_complete(),
        exceeded = fragmentizer.is_max_message_size_exceeded(),
        poisoned = fragmentizer.is_message_poisoned(),
    );

    Some(fragment_info)
}

#[instrument(name = "fragment", skip_all, fields(action = "write"))]
pub fn write_fragment(
    write_buffer: &mut Vec<u8>,
    fragment_data: &[u8],
    fragment_kind: FragmentKind,
) {
    write_buffer.extend(fragment_data);

    trace!(
        type = ?fragment_kind,
        data = escape_byte_string(fragment_data),
    );
}
