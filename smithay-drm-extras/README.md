# Smithay DRM Extras

This crate contains some extra abstractions and helpers over DRM

- `edid` module is responsible for extraction of information from DRM connectors (`model` and `manufacturer`) 
- `drm_scanner` module contains helpers for detecting connector connected and disconnected events as well as mapping crtc to them.
  - `ConnectorScanner` is responsible for tracking connected/disconnected events.
  - `CrtcMapper` trait and `SimpleCrtcMapper` are meant for mapping crtc to connector.
  - `DrmScanner<CrtcMapper>` combines two above into single abstraction. If it does not fit your needs you can always drop down to using `ConnectoScanner` alone.