SELECT hk: sha256('birds|' || species_code), common_name, _source_file, record_hash: _record_hash
FROM staging.stg_birds__fieldbook_species;
