# Changelog

All notable changes to this crate are documented in this file.

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this crate adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.3]

- Extracted schema/config types into `mechanics-config` and now depend on
  `mechanics-config = "0.1.0"`.
- Added Boa GC wrapper newtypes over extracted schema types via
  `#[unsafe_ignore_trace]`.
- Preserved compatibility by re-exporting endpoint/config types at
  `mechanics_core::endpoint::*` and `mechanics_core::job::MechanicsConfig`.

## [0.2.2]

Current published baseline. Git history is the authoritative
record for this and earlier releases; future releases will be
documented going forward in this file.
