//! A corpus replayed through Heartwood's store.
//!
//! The L2 replay commits every block with the store's chunk writer in a temporary database, and
//! checks the store anchors: the stored round rows, missed rows and block columns against the
//! reference node's own rows in the corpus. The kill test replays a corpus in a child process
//! that is killed at random moments, and checks that each restore lands on a committed tip whose
//! state digest equals the continuous replay's.
