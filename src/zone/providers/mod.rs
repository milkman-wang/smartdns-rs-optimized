mod ddr;
mod identity;
pub use ddr::lookup as ddr_lookup;
mod local_ptr;
mod rule;

pub use identity::IdentityZoneProvider;
pub use local_ptr::LocalPtrZoneProvider;
pub use rule::RuleZoneProvider;
