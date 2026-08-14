-- Three values the game says that these types had no label for.
--
-- The second Powerplay renamed what a system stands at, the Trailblazers
-- colonisation update gave stations a service for claiming a system, and
-- megaconstruction sites arrived with a government of their own. A feed
-- carrying any of them failed to parse, and the whole message was dropped
-- with it, so a single new label cost every system and body in that message.
--
-- `powerplay_state` is not among these because it is parsed and never stored.
ALTER TYPE allegiance ADD VALUE IF NOT EXISTS 'FrontlineSolutions';
ALTER TYPE government ADD VALUE IF NOT EXISTS 'Megaconstruction';
ALTER TYPE service ADD VALUE IF NOT EXISTS 'RegisteringColonisation';
