# Security policy

OpenATCA is research software. It is not approved for air traffic control or navigation, and no
operational deployment exists. Security and safety reports are still taken seriously: the
project is built to be held to an operational standard.

## Reporting

Email **connor@avilalabs.org** with "OpenATCA security" in the subject. Please do not open a
public issue for a vulnerability until it is fixed.

Useful to include:

- the commit or release you tested;
- the input that triggers the problem (a recording excerpt, message bytes or a fuzz case), or
  steps to reproduce;
- what you expected and what happened;
- whether you believe it is exploitable deliberately, for example by a forged ADS-B report.

Do not send non-public FAA data, Sensitive Security Information or personal data. If a
reproduction needs such data, describe it and we will arrange a safe way.

## What happens next

- We aim to acknowledge a report within seven days.
- Each report gets an identifier, a severity and a regression test once confirmed.
- Fixes are published with credit to the reporter unless you ask to stay anonymous.

## Safety hazards

A flaw that could cause wrong surveillance, a missed or false alert, or loss of a track for
anyone running OpenATCA is a safety hazard, not only a security issue. Safety hazards are
disclosed to every known user as soon as they are confirmed, with a workaround where one exists.
They are never held back to wait for a release.

## Scope

In scope: everything in this repository, including the decoders for public formats, the
tracker, the replay and benchmark tools, and the display.

Out of scope: third-party data sources (FAA SWIM, adsb.lol, OpenSky) and their availability.
