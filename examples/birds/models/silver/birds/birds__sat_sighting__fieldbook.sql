SELECT
    hk: sha256('birds|' || species_code),
    bird_count,
    bird_count_raw,
    seen_at,
    region,
    _source_file,
    record_hash: _record_hash,
FROM staging.stg_birds__fieldbook_sighting;
