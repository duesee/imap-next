use imap_codec::imap_types::{
    core::NonEmptyVec,
    response::{Capability, Code, Data, Greeting, Status},
};
use tracing::warn;

pub enum ControlFlow {
    Continue,
    Abort,
}

// Remove unsupported capabilities in a greetings `Code::Capability`.
pub fn filter_capabilities_in_greeting(greeting: &mut Greeting) {
    if let Some(Code::Capability(capabilities)) = &mut greeting.code {
        let filtered = filter_capabilities(capabilities.clone());

        if *capabilities != filtered {
            warn!(
                before=?capabilities,
                after=?filtered.clone(),
                "Filtered capabilities in greeting",
            );
            *capabilities = filtered;
        }
    }
}

// Remove unsupported capabilities in a `Data::Capability`.
pub fn filter_capabilities_in_data(data: &mut Data) {
    if let Data::Capability(capabilities) = data {
        let filtered = filter_capabilities(capabilities.clone());

        if *capabilities != filtered {
            warn!(
                before=?capabilities,
                after=?filtered,
                "Filtered capabilities in data",
            );
            *capabilities = filtered;
        }
    }
}

// Remove unsupported capabilities in a status' `Code::Capability`.
pub fn filter_capabilities_in_status(status: &mut Status) {
    if let Status::Ok {
        code: Some(Code::Capability(capabilities)),
        ..
    }
    | Status::No {
        code: Some(Code::Capability(capabilities)),
        ..
    }
    | Status::Bad {
        code: Some(Code::Capability(capabilities)),
        ..
    }
    | Status::Bye {
        code: Some(Code::Capability(capabilities)),
        ..
    } = status
    {
        let filtered = filter_capabilities(capabilities.clone());

        if *capabilities != filtered {
            warn!(
                before=?capabilities,
                after=?filtered,
                "Filtered capabilities in status",
            );
            *capabilities = filtered;
        }
    }
}

// Remove unsupported capabilities in a capability list.
fn filter_capabilities(capabilities: NonEmptyVec<Capability>) -> NonEmptyVec<Capability> {
    let filtered: Vec<_> = capabilities
        .into_iter()
        // TODO: We want to support other AUTH mechanisms, too. AUTH=PLAIN at least.
        //       However, for this to work, the proxy needs to learn how to handle
        //       IMAP authentication flows. This *should* be the last remaining
        //       difficulty we (probably) want to solve for basic IMAP interop.
        .filter(|capability| matches!(capability, Capability::Imap4Rev1))
        .collect();

    NonEmptyVec::try_from(filtered).unwrap_or(NonEmptyVec::from(Capability::Imap4Rev1))
}
