//! The element paths and attributes of a FIXM 3.0 + FAA NAS flight message that this crate has
//! seen in real SFDPS data (see docs/SFDPS_SURVEY.md), relative to the `message` element. A path
//! or attribute outside this table is "unknown" and is counted, never an error. Structure only.

pub(crate) const KNOWN: &[(&str, &[&str])] = &[
    ("", &[]),
    (
        "flight",
        &["centre", "flightType", "source", "system", "timestamp"],
    ),
    ("flight/agreed", &[]),
    (
        "flight/agreed/route",
        &[
            "atcIntendedRoute",
            "initialFlightRules",
            "localIntendedRoute",
            "nasRouteText",
        ],
    ),
    (
        "flight/agreed/route/adaptedArrivalDepartureRoute",
        &["nasRouteAlphanumeric", "nasRouteIdentifier"],
    ),
    (
        "flight/agreed/route/adaptedDepartureRoute",
        &["nasRouteAlphanumeric", "nasRouteIdentifier"],
    ),
    ("flight/agreed/route/estimatedElapsedTime", &["elapsedTime"]),
    ("flight/agreed/route/estimatedElapsedTime/location", &[]),
    (
        "flight/agreed/route/estimatedElapsedTime/location/longitude",
        &[],
    ),
    (
        "flight/agreed/route/estimatedElapsedTime/location/point",
        &["fix"],
    ),
    (
        "flight/agreed/route/estimatedElapsedTime/location/point/location",
        &["srsName"],
    ),
    (
        "flight/agreed/route/estimatedElapsedTime/location/point/location/pos",
        &[],
    ),
    (
        "flight/agreed/route/estimatedElapsedTime/location/region",
        &["airspaceType"],
    ),
    ("flight/agreed/route/expandedRoute", &[]),
    (
        "flight/agreed/route/expandedRoute/routePoint",
        &["estimatedTime"],
    ),
    (
        "flight/agreed/route/expandedRoute/routePoint/point",
        &["fix"],
    ),
    (
        "flight/agreed/route/expandedRoute/routePoint/point/distance",
        &["uom"],
    ),
    (
        "flight/agreed/route/expandedRoute/routePoint/point/location",
        &["srsName"],
    ),
    (
        "flight/agreed/route/expandedRoute/routePoint/point/location/pos",
        &[],
    ),
    (
        "flight/agreed/route/expandedRoute/routePoint/point/radial",
        &["uom"],
    ),
    (
        "flight/agreed/route/nasadaptedArrivalRoute",
        &["nasRouteAlphanumeric", "nasRouteIdentifier"],
    ),
    (
        "flight/agreed/route/nasadaptedArrivalRoute/nasFavNumber",
        &[],
    ),
    (
        "flight/aircraftDescription",
        &[
            "aircraftAddress",
            "aircraftPerformance",
            "aircraftQuantity",
            "equipmentQualifier",
            "registration",
            "tfmsSpecialAircraftQualifier",
            "wakeTurbulence",
        ],
    ),
    ("flight/aircraftDescription/accuracy", &[]),
    (
        "flight/aircraftDescription/accuracy/cmsFieldType",
        &["phase", "type", "uom"],
    ),
    ("flight/aircraftDescription/aircraftType", &[]),
    (
        "flight/aircraftDescription/aircraftType/icaoModelIdentifier",
        &[],
    ),
    (
        "flight/aircraftDescription/capabilities",
        &["standardCapabilities"],
    ),
    (
        "flight/aircraftDescription/capabilities/communication",
        &[
            "otherCommunicationCapabilities",
            "otherDataLinkCapabilities",
            "selectiveCallingCode",
        ],
    ),
    (
        "flight/aircraftDescription/capabilities/communication/communicationCode",
        &[],
    ),
    (
        "flight/aircraftDescription/capabilities/communication/dataLinkCode",
        &[],
    ),
    (
        "flight/aircraftDescription/capabilities/navigation",
        &["otherNavigationCapabilities"],
    ),
    (
        "flight/aircraftDescription/capabilities/navigation/navigationCode",
        &[],
    ),
    (
        "flight/aircraftDescription/capabilities/navigation/performanceBasedCode",
        &[],
    ),
    (
        "flight/aircraftDescription/capabilities/surveillance",
        &["otherSurveillanceCapabilities"],
    ),
    (
        "flight/aircraftDescription/capabilities/surveillance/surveillanceCode",
        &[],
    ),
    ("flight/arrival", &["arrivalPoint"]),
    ("flight/arrival/arrivalAerodrome", &[]),
    ("flight/arrival/arrivalAerodrome/point", &[]),
    (
        "flight/arrival/arrivalAerodrome/point/location",
        &["srsName"],
    ),
    ("flight/arrival/arrivalAerodrome/point/location/pos", &[]),
    ("flight/arrival/arrivalAerodromeAlternate", &["code"]),
    ("flight/arrival/runwayPositionAndTime", &[]),
    ("flight/arrival/runwayPositionAndTime/runwayTime", &[]),
    (
        "flight/arrival/runwayPositionAndTime/runwayTime/actual",
        &["time"],
    ),
    (
        "flight/arrival/runwayPositionAndTime/runwayTime/estimated",
        &["time"],
    ),
    ("flight/assignedAltitude", &[]),
    ("flight/assignedAltitude/altFixAlt", &[]),
    ("flight/assignedAltitude/altFixAlt/point", &["fix"]),
    ("flight/assignedAltitude/altFixAlt/post", &["uom"]),
    ("flight/assignedAltitude/altFixAlt/pre", &["uom"]),
    ("flight/assignedAltitude/block", &[]),
    ("flight/assignedAltitude/block/above", &["uom"]),
    ("flight/assignedAltitude/block/below", &["uom"]),
    ("flight/assignedAltitude/simple", &["uom"]),
    ("flight/assignedAltitude/vfr", &[]),
    ("flight/assignedAltitude/vfrPlus", &["uom"]),
    (
        "flight/controllingUnit",
        &["sectorIdentifier", "unitIdentifier"],
    ),
    (
        "flight/coordination",
        &["coordinationTime", "coordinationTimeHandling"],
    ),
    ("flight/coordination/coordinationFix", &["fix"]),
    ("flight/coordination/coordinationFix/distance", &["uom"]),
    ("flight/coordination/coordinationFix/location", &["srsName"]),
    ("flight/coordination/coordinationFix/location/pos", &[]),
    ("flight/coordination/coordinationFix/radial", &["uom"]),
    ("flight/departure", &["departurePoint"]),
    ("flight/departure/runwayPositionAndTime", &[]),
    ("flight/departure/runwayPositionAndTime/runwayTime", &[]),
    (
        "flight/departure/runwayPositionAndTime/runwayTime/actual",
        &["time"],
    ),
    (
        "flight/departure/runwayPositionAndTime/runwayTime/estimated",
        &["time"],
    ),
    ("flight/departure/takeoffAlternateAerodrome", &["code"]),
    ("flight/enRoute", &[]),
    ("flight/enRoute/alternateAerodrome", &["code"]),
    ("flight/enRoute/beaconCodeAssignment", &[]),
    ("flight/enRoute/beaconCodeAssignment/currentBeaconCode", &[]),
    (
        "flight/enRoute/beaconCodeAssignment/reassignedBeaconCode",
        &[],
    ),
    ("flight/enRoute/boundaryCrossings", &[]),
    ("flight/enRoute/boundaryCrossings/handoff", &["event"]),
    (
        "flight/enRoute/boundaryCrossings/handoff/acceptingUnit",
        &["sectorIdentifier", "unitIdentifier"],
    ),
    (
        "flight/enRoute/boundaryCrossings/handoff/receivingUnit",
        &["sectorIdentifier", "unitIdentifier"],
    ),
    (
        "flight/enRoute/boundaryCrossings/handoff/transferringUnit",
        &["sectorIdentifier", "unitIdentifier"],
    ),
    ("flight/enRoute/cleared", &["clearanceHeading"]),
    ("flight/enRoute/pointout", &[]),
    (
        "flight/enRoute/pointout/originatingUnit",
        &["sectorIdentifier", "unitIdentifier"],
    ),
    (
        "flight/enRoute/pointout/receivingUnit",
        &["sectorIdentifier", "unitIdentifier"],
    ),
    (
        "flight/enRoute/position",
        &[
            "coastIndicator",
            "positionTime",
            "reportSource",
            "targetPositionTime",
        ],
    ),
    ("flight/enRoute/position/actualSpeed", &[]),
    ("flight/enRoute/position/actualSpeed/surveillance", &["uom"]),
    ("flight/enRoute/position/altitude", &["uom"]),
    ("flight/enRoute/position/position", &[]),
    ("flight/enRoute/position/position/location", &["srsName"]),
    ("flight/enRoute/position/position/location/pos", &[]),
    (
        "flight/enRoute/position/targetAltitude",
        &["invalid", "uom"],
    ),
    ("flight/enRoute/position/targetPosition", &["srsName"]),
    ("flight/enRoute/position/targetPosition/pos", &[]),
    ("flight/enRoute/position/trackVelocity", &[]),
    ("flight/enRoute/position/trackVelocity/x", &["uom"]),
    ("flight/enRoute/position/trackVelocity/y", &["uom"]),
    (
        "flight/flightIdentification",
        &["aircraftIdentification", "computerId", "siteSpecificPlanId"],
    ),
    (
        "flight/flightIdentificationPrevious",
        &["aircraftIdentification", "computerId", "siteSpecificPlanId"],
    ),
    ("flight/flightPlan", &["flightPlanRemarks", "identifier"]),
    ("flight/flightStatus", &["fdpsFlightStatus"]),
    ("flight/gufi", &["codeSpace"]),
    ("flight/interimAltitude", &["uom"]),
    ("flight/operator", &[]),
    ("flight/operator/operatingOrganization", &[]),
    (
        "flight/operator/operatingOrganization/organization",
        &["name"],
    ),
    ("flight/originator", &[]),
    ("flight/originator/aftnAddress", &[]),
    ("flight/requestedAirspeed", &[]),
    ("flight/requestedAirspeed/nasAirspeed", &["uom"]),
    ("flight/requestedAltitude", &[]),
    ("flight/requestedAltitude/simple", &["uom"]),
    ("flight/requestedAltitude/vfr", &[]),
    ("flight/requestedAltitude/vfrPlus", &["uom"]),
    ("flight/routeToRevisedDestination", &[]),
    ("flight/routeToRevisedDestination/route", &["routeText"]),
    ("flight/specialHandling", &[]),
    ("flight/supplementalData", &[]),
    ("flight/supplementalData/additionalFlightInformation", &[]),
    (
        "flight/supplementalData/additionalFlightInformation/nameValue",
        &["name", "value"],
    ),
];
