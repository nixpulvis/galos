CREATE TYPE StationType AS ENUM (
    'AsteroidBase',
    'Coriolis',
    'CraterOutpost',
    'CraterPort',
    'FleetCarrier',
    'MegaShip',
    'Ocellus',
    'Orbis',
    'Outpost'
);


CREATE TYPE Service AS ENUM (
    'Autodock',
    'Blackmarket',
    'CarrierFuel',
    'CarrierManagement',
    'Commodities',
    'Contacts',
    'CrewLounge',
    'Dock',
    'Engineer',
    'Exploration',
    'Facilitator',
    'FlightController',
    'Initiatives',
    'MaterialTrader',
    'Missions',
    'MissionsGenerated',
    'Modulepacks',
    'Outfitting',
    'Powerplay',
    'Rearm',
    'Refuel',
    'Repair',
    'SearchRescue',
    'Shipyard',
    'Shop',
    'StationMenu',
    'StationOperations',
    'TechBroker',
    'Tuning',
    'VoucherRedemption',
    'Livery',
    'SocialSpace',
    'Bartender',
    'VistaGenomics',
    'PioneerSupplies',
    'ApexInterstellar',
    'FrontlineSolutions'
);

CREATE TYPE EconomyShare AS (
    name        Economy,
    proportion  double precision
);

CREATE TYPE LandingPads AS (
    large   smallint,
    medium  smallint,
    small   smallint
);

CREATE TABLE stations (
    system_address     bigint      REFERENCES systems  NOT NULL,
    name               varchar     NOT NULL,
    ty                 StationType NOT NULL,
    dist_from_star_ls  double precision,
    market_id          bigint,
    landing_pads       LandingPads,
    faction            varchar,
    government         Government,
    allegiance         Allegiance,
    services           Service[],
    economies          EconomyShare[],
    updated_at         timestamp  NOT NULL,
    updated_by         varchar    NOT NULL,


    FOREIGN KEY (system_address) REFERENCES systems (address),
    PRIMARY KEY(system_address, name)
);
