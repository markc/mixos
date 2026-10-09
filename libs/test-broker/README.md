# test-broker

Development fixture for the Term-to-Mix native session acceptance tests.
It includes the actual noded modules from their owning service and a copy of
Term's sealed-FD implementation (`src/session_fd.rs`, from term-core; it
returns to term-core when Term is ported to egui). It has no broker copy or
simulated session transport. The fixture controls an isolated broker runtime,
verified Unix ingress, restart and pause boundaries.
