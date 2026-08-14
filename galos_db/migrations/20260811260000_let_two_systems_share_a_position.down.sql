-- Fails where two systems have come to share a position, which is the point.
ALTER TABLE systems ADD CONSTRAINT systems_position_key UNIQUE (position);
