//! Pure, bounded scenario intent, traffic materialization and turn geometry.

mod scenario;
mod traffic;
mod turn;

pub use scenario::{
    generate, normal_arrival_slot, PerformanceProfileRef, ProcedureKind, ProcedureRouteRef,
    RouteSource, RouteVariant, ScenarioError, ScenarioRequest, SyntheticFlightPlan,
    SyntheticScenario, B738_NG_MMO_MILLI, B738_NG_VMO_KCAS, EVENT_TIME_UNIT, MAX_DURATION_MS,
    NORMAL_ORD_AIRCRAFT, NORMAL_ORD_DURATION_MS, NORMAL_ORD_SEED, SCENARIO_FORMAT_VERSION,
};
pub use traffic::{
    crop_arrival_area, materialize_traffic, materialize_traffic_with_altitude_preferences,
    validate_route_motion, MotionAssumptions, ResolvedFix, ResolvedRoute, SyntheticReport,
    TrafficError, VerticalGuidance, MAX_RESOLVED_ROUTE_FIXES, MAX_SYNTHETIC_REPORTS,
};
pub use turn::{
    build_fly_by_fillet, build_fly_by_route, radius_for_turn_rate, FlyByError, FlyByFillet,
    FlyByRoutePath, GeoPoint, RoutePathPoint, RoutePointKind, RouteWaypoint, WaypointTurnMode,
};
