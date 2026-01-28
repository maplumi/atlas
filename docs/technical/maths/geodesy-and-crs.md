# Geodesy and CRS

Atlas uses WGS84 and transforms through:
Geodetic → ECEF → Local Tangent → Camera Space → Clip Space

CRS transformations must be explicit and invertible within tolerance.
