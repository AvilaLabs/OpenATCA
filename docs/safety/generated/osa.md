<!-- Generated from docs/safety/hazard-log.json and its oa-trace references. -->

**Assessment status: DRAFT_NOT_ASSESSED | Coverage status: DECLARED_FUNCTION_SCOPE_ONLY | Not an accredited SSA.**

| Function | Hazard ID | Hazard |
| --- | --- | --- |
| navigation_and_adaptation_data | HZ-AIRSPACE-001 | A route, boundary, or facility constraint may be misplaced or missing from downstream display and warning functions. |
| bounded_archive_and_compressed_input | HZ-ARCHIVE-001 | Downstream facility, terrain, obstacle, or flight input may be missing or misleading. |
| conflict_alert | HZ-CA-001 | The controller may not receive a timely warning of a developing loss of separation. |
| acc_to_acc_flight_coordination_interface | HZ-COORDINATION-001 | A peer facility may receive missing or misleading coordination information, causing an incorrect or stale flight-plan/handoff picture. |
| migration_selector_and_rollback | HZ-CUTOVER-001 | A migration may change output or lose state while legacy output is not safely recoverable. |
| traffic_and_safety_status_display | HZ-DISPLAY-001 | A controller may act on an incomplete picture or fail to recognize a warning limitation. |
| migration_comparison_and_safety_evidence | HZ-EVIDENCE-001 | A reviewer may receive an incomplete basis for a migration decision. |
| surveillance_and_structured_feed_ingestion | HZ-FEED-001 | Feed-derived aircraft or status state may be missing, stale, or misleading. |
| flight_plan_and_coordination_message_ingestion | HZ-FLIGHTDATA-001 | Flight identity, route, altitude, or coordination state may be incorrect or incomplete. |
| flight_state_updates_and_trajectory_presentation | HZ-FLIGHTOPS-001 | The presented trajectory or current flight state can be misleading during coordination. |
| geodetic_and_navigation_geometry | HZ-GEOMETRY-001 | An incorrect position, separation, or geometric boundary can mislead association, routing, or alert calculations. |
| minimum_safe_altitude_warning | HZ-MSAW-001 | A terrain or obstacle warning may be absent or misleading during descent or approach. |
| safety_net_state_and_display | HZ-NET-STATE-001 | A controller may believe a warning function is active when it is not, or misunderstand why an alert was suppressed. |
| structured_weather_and_trajectory_input | HZ-STRUCTURED-INPUT-001 | Weather or trajectory input consumed by downstream adapters may be incorrect, missing, or treated as known when it is not. |
| plot_to_track_association_and_identity | HZ-SURV-ASSOC-001 | Aircraft identity or trajectory continuity can be silently swapped or fragmented. |
| surveillance_wire_measurement_decoding | HZ-SURV-INPUT-001 | The tracker may receive a wrong position, altitude, identity, or time. |
| surveillance_track_state_and_numeric_integrity | HZ-SURV-STATE-001 | Track position, motion, uncertainty, or availability becomes incorrect or silently absent. |
| surveillance_time_gates_and_track_lifecycle | HZ-SURV-TIME-001 | Traffic state is stale, late, or missing when used by display or alert functions. |
| terrain_obstacle_and_minimum_altitude_data | HZ-TERRAIN-001 | A terrain or obstacle warning may be absent or use an unsafe floor. |
| weather_and_pressure_altitude_inputs | HZ-WEATHER-001 | Pressure correction or hazard context used by warning/display logic may be wrong or unavailable. |
