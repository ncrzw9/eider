SELECT DISTINCT hk: sha256('birds|' || species_code), business_key: species_code, domain: 'birds'
FROM staging.stg_birds__fieldbook_species;
