SELECT
    species_code: TRIM(species_code),
    bird_count: TRY_CAST(bird_count AS integer),
    bird_count_raw: bird_count,
    seen_at,
    region,
    _source_file,
    _record_hash,
FROM bronze.fieldbook__sightings;
