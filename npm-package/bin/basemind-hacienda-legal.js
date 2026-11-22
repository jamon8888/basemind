#!/usr/bin/env node
// Explicit command name for the confidential profile. It selects the
// `hacienda-legal` release asset and never falls back to the full binary.
process.env.BASEMIND_PROFILE = "hacienda-legal";
require("./basemind.js");
