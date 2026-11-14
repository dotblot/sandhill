def _sandhill_bio_check_impl(request, namespace):
    """Read-only, full-block AnnData checks; definitions only, never auto-invoked.

    Request: checkpoint, object (plain identifier), counts_source {kind:raw_counts,
    processing_state:raw, provenance, layer}, species, gene_id_type, source_files,
    parameters, design, details, baseline. Baselines are opaque daemon-journal data.
    Only a matching, unchanged, read-only .h5ad source establishes correspondence;
    details.source_counts_layer selects its counts (defaults to counts_source.layer).
    No matrix snapshot or list of original identifiers is placed in the baseline.

    details.qc_metrics maps total_counts/n_genes/pct_mito to obs columns.
    details.qc_thresholds: [{metric,min?,max?,reason}], inclusive bounds.
    details.qc_exclusions_file: CSV axis(cell|gene),id,reason, exactly removed IDs.
    details.doublets: {status:performed|not_performed,reason}; declarations only.
    details.mito_gene_ids: explicit feature IDs (required for Ensembl mitochondrial QC).
    Result details: result_file,cluster_key,embedding_key,expression_layer,
    marker_file,annotation_file,marker_method,p_adjust_method,code_files,
    execution_run_ids. The daemon alone injects request.execution_evidence from its
    successful same-session journal runs; details cannot provide trusted evidence.
    Tables are UTF-8 CSV; annotation may be JSON list of rows.
    Marker columns: cluster,gene,p_val_adj,pct_expr,mean_expr (optional p_val).
    Annotation: cluster,label,gene,pct_expr,mean_expr,evidence_type(support|conflict),
    source,status(confirmed|pending|unknown),reason. Fractions are 0..1; arithmetic
    means include zeros, in the explicit expression layer and cluster. At least two
    support genes per cluster; conflict rows or a substantive no-conflict reason.
    'confirmed' is a human assertion, not a biological correctness result.
    details.parameter_evidence: pca/neighbors/clustering/umap ->
    {uns_key,parameters:{...},seed:integer}. Emitted uns[key].params are compared;
    non-emitted values are explicitly recorded declarations accompanying actual
    executed code, not independently measured hidden RNG/library arguments.
    Full-family raw pvals in marker_uns_key (default rank_genes_groups) permit
    independent BH/Bonferroni recomputation; truncated families are not_checked.
    """
    import csv
    import hashlib
    import importlib.metadata
    import json
    import math
    import os
    import platform
    import re
    import sys

    required = {
        'input': ('object', 'identifiers', 'metadata', 'counts_source',
                  'counts_values', 'processing_state', 'environment', 'seed',
                  'input_sources', 'gene_species', 'study_design'),
        'qc': ('qc_metrics', 'qc_thresholds', 'qc_exclusions', 'qc_subset',
               'qc_counts_unchanged', 'qc_metadata', 'qc_doublets'),
        'result': ('result_file', 'result_metadata', 'result_clusters',
                   'result_embeddings', 'result_parameters', 'result_markers',
                   'result_annotation', 'result_design', 'human_review'),
    }
    checkpoint = request.get('checkpoint') if isinstance(request, dict) else None
    items, facts = {}, {'checkpoint': checkpoint, 'checker': 'sandhill-anndata-v1'}
    result = {'items': [], 'facts': facts}
    handles = []

    def add(key, status, message, evidence=None):
        item = {'id': key, 'status': status, 'message': str(message)[:1600]}
        if evidence is not None:
            item['evidence'] = evidence
        items[key] = item

    def finish():
        for key in required.get(checkpoint, ()):
            if key not in items:
                add(key, 'not_checked', 'Prerequisite unavailable; see preceding checks.')
        result['items'] = list(items.values())
        for handle in handles:
            try:
                handle.file.close()
            except Exception:
                pass
        try:
            oversized = len(json.dumps(result, allow_nan=False).encode('utf-8')) > 768 * 1024
        except (TypeError, ValueError):
            oversized = True
        if oversized:
            result.pop('baseline', None)
            result['facts'] = {'checkpoint': checkpoint, 'checker': 'sandhill-anndata-v1',
                               'summary_limit': 'Evidence exceeded JSON-safe bounded summary limit'}
            if 'input_sources' in items:
                add('input_sources', 'not_checked', 'Compact baseline exceeds bounded summary limit; no baseline retained')
            result['items'] = [{k: v for k, v in item.items() if k != 'evidence'}
                               for item in items.values()]
        return result

    def text(value):
        return isinstance(value, str) and bool(value.strip())

    def digest_ids(index):
        h = hashlib.sha256()
        for value in index:
            data = str(value).encode('utf-8')
            h.update(len(data).to_bytes(8, 'big'))
            h.update(data)
        return h.hexdigest()

    def file_digest(path):
        h = hashlib.sha256()
        with open(path, 'rb') as stream:
            while True:
                chunk = stream.read(1024 * 1024)
                if not chunk:
                    break
                h.update(chunk)
        return h.hexdigest()

    class EvidenceUnavailable(ValueError):
        pass

    def file_info(path):
        if not isinstance(path, str) or not os.path.isabs(path):
            raise EvidenceUnavailable('Expected an absolute file path')
        if not os.path.isfile(path):
            raise EvidenceUnavailable('File does not exist: ' + path[:200])
        before = os.stat(path)
        value = {'path': path, 'sha256': file_digest(path), 'bytes': before.st_size}
        after = os.stat(path)
        if (before.st_size, before.st_mtime_ns, before.st_ino) != (
                after.st_size, after.st_mtime_ns, after.st_ino):
            raise ValueError('File changed while hashing')
        return value

    def layer(obj, name):
        if not text(name):
            raise ValueError('Counts/expression layer must be explicit')
        return obj.X if name == 'X' else obj.layers[name]

    def slice_rows(matrix, start, stop):
        if hasattr(matrix, 'iloc'):
            return matrix.iloc[start:stop, :].to_numpy(copy=False)
        return matrix[start:stop, :]

    def blocks(matrix, source_width=0):
        # Include the original source width: gene filtering must not turn a
        # narrow retained block into an unbounded original-source allocation.
        width = max(1, int(matrix.shape[1]), source_width)
        step = max(1, min(256, 262144 // width))
        for start in range(0, matrix.shape[0], step):
            yield start, slice_rows(matrix, start, min(start + step, matrix.shape[0]))

    def values(block):
        return block.data if sparse.issparse(block) else np.asarray(block)

    def inspect_counts(matrix, mito=None):
        n = matrix.shape[0]
        total = np.zeros(n, dtype=np.float64)
        genes = np.zeros(n, dtype=np.int64)
        mt = np.zeros(n, dtype=np.float64) if mito is not None else None
        bad = {'nonfinite': 0, 'negative': 0, 'noninteger': 0}
        for start, block in blocks(matrix):
            data = values(block)
            finite = np.isfinite(data)
            bad['nonfinite'] += int(np.count_nonzero(~finite))
            bad['negative'] += int(np.count_nonzero(data < 0))
            bad['noninteger'] += int(np.count_nonzero(
                finite & (np.abs(data - np.rint(data)) > 1e-6)))
            if sparse.issparse(block):
                block = block.tocsr(copy=False)
                if not block.has_canonical_format:
                    block.sum_duplicates()  # only this private sliced block
                total[start:start + block.shape[0]] = np.asarray(block.sum(axis=1)).ravel()
                genes[start:start + block.shape[0]] = np.asarray((block > 0).sum(axis=1)).ravel()
                if mt is not None:
                    mt[start:start + block.shape[0]] = np.asarray(block[:, mito].sum(axis=1)).ravel()
            else:
                total[start:start + block.shape[0]] = np.sum(block, axis=1, dtype=np.float64)
                genes[start:start + block.shape[0]] = np.count_nonzero(block > 0, axis=1)
                if mt is not None:
                    mt[start:start + block.shape[0]] = np.sum(block[:, mito], axis=1)
        metrics = {'total_counts': total, 'n_genes': genes}
        if mt is not None:
            pct = np.full(n, np.nan)
            np.divide(100 * mt, total, out=pct, where=total > 0)
            metrics['pct_mito'] = pct
        return bad, metrics

    def summary(array):
        finite = np.asarray(array)[np.isfinite(array)]
        return {'n': int(len(array)), 'finite': int(len(finite)),
                'min': float(finite.min()) if finite.size else None,
                'max': float(finite.max()) if finite.size else None,
                'mean': float(finite.mean()) if finite.size else None}

    def same_matrix(left, right, row_ids=None, col_ids=None):
        if row_ids is None and left.shape != right.shape:
            return False
        for start, block in blocks(left, right.shape[1]):
            stop = start + block.shape[0]
            if row_ids is None:
                other = slice_rows(right, start, stop)
            else:
                # Backed AnnData requires sorted fancy indices. Read sorted source
                # rows, then restore live order; never materialize the whole source.
                selected = row_ids[start:stop]
                order = np.argsort(selected)
                other = right[selected[order], :]
                other = other[np.argsort(order), :]
                other = other[:, col_ids]
            if sparse.issparse(block) or sparse.issparse(other):
                delta = sparse.csr_matrix(block) - sparse.csr_matrix(other)
                if delta.nnz and not np.all(delta.data == 0):
                    return False
            elif not np.array_equal(np.asarray(block), np.asarray(other), equal_nan=True):
                return False
        return True

    def same_frame(left, right, columns, allow_pruned_categories=False):
        # AnnData legitimately drops unused categories while filtering. Values
        # and retained category ordering must still agree with the input source.
        for column in columns:
            if column not in left or column not in right:
                return False
            a, b = left[column], right[column]
            if allow_pruned_categories and hasattr(a, 'cat') and hasattr(b, 'cat'):
                a, b = a.cat.remove_unused_categories(), b.cat.remove_unused_categories()
            if not a.index.equals(b.index) or not a.equals(b):
                return False
        return True

    def qc_metadata_matches(original_obs, original_var, obs_columns, var_columns):
        # Only fixed Scanpy-derived fields are replaceable, and each is checked
        # independently. A caller cannot exempt donor/sample metadata by mapping
        # its name in details.qc_metrics.
        cell_values = {}
        if metrics is not None:
            cell_values = {'total_counts': metrics['total_counts'],
                           'n_genes_by_counts': metrics['n_genes']}
            if 'pct_mito' in metrics:
                cell_values['pct_counts_mt'] = metrics['pct_mito']
                cell_values['total_counts_mt'] = metrics['pct_mito'] * metrics['total_counts'] / 100
            for key in ('total_counts', 'n_genes_by_counts', 'total_counts_mt'):
                if key in cell_values:
                    cell_values['log1p_' + key] = np.log1p(cell_values[key])
        cell_derived = {'total_counts', 'n_genes_by_counts', 'total_counts_mt', 'pct_counts_mt',
                        'log1p_total_counts', 'log1p_n_genes_by_counts', 'log1p_total_counts_mt'}
        gene_derived = {'n_cells_by_counts', 'mean_counts', 'pct_dropout_by_counts', 'total_counts',
                        'log1p_mean_counts', 'log1p_total_counts'}
        gene_values = {}
        if counts is not None and any(c in gene_derived for c in var_columns):
            totals = np.zeros(obj.n_vars, dtype=np.float64)
            detected = np.zeros(obj.n_vars, dtype=np.int64)
            for _, block in blocks(counts):
                totals += np.asarray(block.sum(axis=0)).ravel()
                detected += np.asarray((block > 0).sum(axis=0)).ravel()
            means = totals / obj.n_obs
            gene_values = {'n_cells_by_counts': detected, 'mean_counts': means,
                           'pct_dropout_by_counts': 100 * (1 - detected / obj.n_obs),
                           'total_counts': totals, 'log1p_mean_counts': np.log1p(means),
                           'log1p_total_counts': np.log1p(totals)}
        errors, recomputed = [], []
        for axis, live, original, columns, fixed, measured in (
                ('obs', obj.obs, original_obs, obs_columns, cell_derived, cell_values),
                ('var', obj.var, original_var, var_columns, gene_derived, gene_values)):
            for column in columns:
                if column not in fixed:
                    if not same_frame(live, original, [column], True):
                        errors.append(axis + '.' + str(column) + ': original metadata changed')
                    continue
                if column not in live or column not in measured:
                    errors.append(axis + '.' + column + ': derived field unavailable')
                    continue
                try:
                    stored = np.asarray(live[column], dtype=float)
                    expected = measured[column]
                    valid = (np.isfinite(stored).all() and np.isfinite(expected).all()
                             and np.allclose(stored, expected, rtol=1e-5, atol=1e-5))
                except (TypeError, ValueError):
                    valid = False
                if valid:
                    recomputed.append(axis + '.' + column)
                else:
                    errors.append(axis + '.' + column + ': recomputed QC mismatch')
        return not errors, {'recomputed_fields': recomputed[:40], 'issues': errors[:30]}

    def metadata_valid(obj):
        return (len(obj.obs) == obj.n_obs and len(obj.var) == obj.n_vars
                and obj.obs.index.equals(obj.obs_names)
                and obj.var.index.equals(obj.var_names)
                and obj.obs.columns.is_unique and obj.var.columns.is_unique)

    def mito_mask(obj):
        parameters = request.get('parameters') or {}
        explicit = details.get('mito_gene_ids', parameters.get('mito_genes'))
        mapping_source = details.get('mito_source', parameters.get('mito_source'))
        persisted = (request.get('baseline') or {}).get('mitochondrial_mapping')
        from_baseline = explicit is None and isinstance(persisted, dict)
        if from_baseline:
            explicit, mapping_source = persisted.get('gene_ids'), persisted.get('source')
        if explicit is not None:
            if not isinstance(explicit, list) or not explicit or not all(text(x) for x in explicit):
                return None, 'Explicit mitochondrial IDs are empty or invalid'
            if not text(mapping_source):
                return None, 'Explicit mitochondrial mapping requires mito_source provenance'
            missing = set(explicit) - set(obj.var_names)
            if missing and not from_baseline:
                return None, 'Explicit mitochondrial IDs missing from current feature set'
            return np.asarray(obj.var_names.isin(explicit)), 'explicit mapping: ' + mapping_source[:300]
        species = request.get('species')
        if request.get('gene_id_type') != 'symbol' or species not in ('human', 'mouse'):
            return None, 'Mitochondrial genes unknown; supply mitochondrial IDs and species'
        prefix = 'MT-' if species == 'human' else 'mt-'
        mask = np.asarray(obj.var_names.str.startswith(prefix))
        if not mask.any():
            return None, 'No mitochondrial symbols matched; this is unknown, not 0%'
        return mask, 'species-specific symbol prefix ' + prefix

    def reopen(path):
        if not text(path) or not os.path.isabs(path) or not path.lower().endswith('.h5ad'):
            raise ValueError('An absolute existing .h5ad path is required')
        import h5py
        from collections.abc import Mapping

        class DiskMapping(Mapping):
            def __init__(self, group, matrices=False):
                self.group, self.matrices = group, matrices

            def __iter__(self):
                return iter(self.group)

            def __len__(self):
                return len(self.group)

            def __getitem__(self, key):
                value = self.group[key]
                if self.matrices:
                    if isinstance(value, h5py.Dataset):
                        return value
                    encoding = value.attrs.get('encoding-type', '')
                    if encoding in ('csr_matrix', 'csc_matrix'):
                        return ad.io.sparse_dataset(value)
                return ad.io.read_elem(value)

        class DiskObject:
            pass

        # AnnData backed='r' only backs X; read_h5ad eagerly loads layers.
        # Read each matrix lazily instead, including raw counts layers.
        disk = DiskObject()
        disk.file = h5py.File(path, 'r')
        handles.append(disk)
        root = disk.file
        disk.obs, disk.var = ad.io.read_elem(root['obs']), ad.io.read_elem(root['var'])
        disk.obs_names, disk.var_names = disk.obs.index, disk.var.index
        disk.n_obs, disk.n_vars = len(disk.obs), len(disk.var)
        disk.shape = (disk.n_obs, disk.n_vars)
        disk.X = DiskMapping(root, matrices=True)['X'] if 'X' in root else None
        disk.layers = DiskMapping(root['layers'], matrices=True) if 'layers' in root else {}
        disk.obsm = DiskMapping(root['obsm'], matrices=True) if 'obsm' in root else {}
        disk.uns = DiskMapping(root['uns']) if 'uns' in root else {}
        return disk

    def source_from_baseline():
        baseline = request.get('baseline')
        if not isinstance(baseline, dict) or baseline.get('schema') != 'sandhill-anndata-source-v1':
            raise ValueError('No trustworthy input baseline from a completed input check')
        src = baseline['source']
        info = file_info(src['path'])
        if info['sha256'] != src['sha256']:
            raise ValueError('Original source file changed since input baseline')
        original = reopen(src['path'])
        expected = baseline['input']
        if (original.n_obs != expected['n_obs'] or original.n_vars != expected['n_vars']
                or digest_ids(original.obs_names) != expected['obs_hash']
                or digest_ids(original.var_names) != expected['var_hash']):
            raise ValueError('Baseline identifiers do not match original source')
        if not original.obs_names.is_unique or not original.var_names.is_unique:
            raise ValueError('Original source identifiers are not unique')
        return baseline, original

    def read_rows(path, fields, allow_json=False, allow_empty=False):
        info = file_info(path)
        # Tables are external evidence, not matrix data. Bound both parsing memory
        # and report size; oversize evidence is explicitly not checked, not sampled.
        if info['bytes'] > 16 * 1024 * 1024:
            raise EvidenceUnavailable('Evidence table exceeds 16 MiB inspection limit; not sampled')
        with open(path, encoding='utf-8-sig', newline='') as stream:
            if allow_json and path.lower().endswith('.json'):
                rows = json.load(stream)
            else:
                reader = csv.DictReader(stream)
                if not set(fields).issubset(reader.fieldnames or []):
                    raise ValueError('Missing table columns: ' + ','.join(sorted(set(fields) - set(reader.fieldnames or []))))
                rows = list(reader)
        if not isinstance(rows, list):
            raise ValueError('Evidence table must be a list of rows')
        if len(rows) > 100000 or (not rows and not allow_empty):
            raise EvidenceUnavailable('Evidence table has no rows or exceeds the full-inspection row limit')
        if not all(isinstance(row, dict) and set(fields).issubset(row) for row in rows):
            raise ValueError('Invalid evidence rows or missing columns')
        return rows, info

    def number(value):
        result = float(value)
        if not math.isfinite(result):
            raise ValueError('Non-finite evidence value')
        return result

    def expression_summaries(matrix, labels, pairs):
        # Group requested genes, then scan every matrix block exactly once. Keep
        # only O(number of evidence rows) aggregates, never a dense expression copy.
        by_gene = {}
        for cluster, gene in pairs:
            by_gene.setdefault(gene, []).append(cluster)
        totals = {pair: [0.0, 0] for pair in pairs}
        sizes = {cluster: int(np.count_nonzero(labels == cluster)) for cluster, _ in pairs}
        genes = list(by_gene)
        positions = obj.var_names.get_indexer(genes)
        if (positions < 0).any():
            raise ValueError('Evidence contains genes absent from live object')
        for start, block in blocks(matrix):
            selected = block[:, positions]
            # Check the whole expression block, not just cited markers.
            if not np.isfinite(values(block)).all():
                raise ValueError('Expression source contains non-finite values')
            if np.any(values(block) < 0):
                raise ValueError('Expression source contains negative values; expression fractions require nonnegative count/log expression')
            block_labels = labels[start:start + block.shape[0]]
            for j, gene in enumerate(genes):
                column = selected[:, [j]]
                for cluster in by_gene[gene]:
                    mask = block_labels == cluster
                    if not mask.any():
                        continue
                    if sparse.issparse(column):
                        subset = column[mask, :]
                        total = float(subset.sum())
                        positive = int((subset > 0).sum())
                    else:
                        subset = np.asarray(column).reshape(-1)[mask]
                        total = float(subset.sum())
                        positive = int(np.count_nonzero(subset > 0))
                    pair = (cluster, gene)
                    totals[pair][0] += total
                    totals[pair][1] += positive
        return {pair: (v[1] / sizes[pair[0]], v[0] / sizes[pair[0]])
                for pair, v in totals.items()}

    def match_summaries(rows, measured):
        mismatches = 0
        for row in rows:
            key = (str(row['cluster']), str(row['gene']))
            pct, mean = measured[key]
            declared_pct, declared_mean = number(row['pct_expr']), number(row['mean_expr'])
            if not 0 <= declared_pct <= 1:
                raise ValueError('pct_expr must be a fraction in [0,1]')
            if not (math.isclose(pct, declared_pct, rel_tol=1e-5, abs_tol=1e-6)
                    and math.isclose(mean, declared_mean, rel_tol=1e-5, abs_tol=1e-6)):
                mismatches += 1
        return mismatches

    try:
        if checkpoint not in required:
            add('request', 'fail', 'Unsupported checkpoint')
            return finish()
        if not isinstance(namespace, dict):
            add('object', 'fail', 'Kernel namespace is unavailable')
            return finish()
        name = request.get('object')
        if not text(name) or not name.isidentifier() or name not in namespace:
            add('object', 'fail', 'Object must be a plain existing namespace identifier')
            return finish()
        try:
            import numpy as np
            import anndata as ad
            from scipy import sparse
        except ImportError as exc:
            add('environment', 'not_checked', 'Missing dependency; install only with approval: ' + str(exc))
            return finish()
        obj = namespace[name]
        if not isinstance(obj, ad.AnnData):
            add('object', 'fail', 'Expected an actual anndata.AnnData object')
            return finish()
        details = request.get('details') or {}
        counts_source = request.get('counts_source') or {}
        if not isinstance(details, dict) or not isinstance(counts_source, dict):
            add('request', 'fail', 'details and counts_source must be mappings')
            return finish()
        facts['object'] = {'name': name, 'class': type(obj).__module__ + '.' + type(obj).__name__,
                           'n_obs': int(obj.n_obs), 'n_vars': int(obj.n_vars),
                           'backed': bool(obj.isbacked), 'view': bool(obj.is_view)}
        facts['declarations'] = {'species': request.get('species'),
                                 'gene_id_type': request.get('gene_id_type'),
                                 'counts_source': {k: str(counts_source.get(k, ''))[:500]
                                                   for k in ('kind', 'processing_state', 'layer', 'provenance')}}
        valid_ids = (obj.n_obs > 0 and obj.n_vars > 0 and obj.obs_names.is_unique
                     and obj.var_names.is_unique
                     and all(text(x) for x in obj.obs_names)
                     and all(text(x) for x in obj.var_names))
        if checkpoint == 'input':
            add('object', 'pass', 'Live AnnData object and dimensions inspected', facts['object'])
            add('identifiers', 'pass' if valid_ids else 'fail',
                'Nonempty unique cell and gene identifiers required',
                {'obs_hash': digest_ids(obj.obs_names), 'var_hash': digest_ids(obj.var_names)})
            add('metadata', 'pass' if metadata_valid(obj) else 'fail',
                'Cell/gene metadata axes and column names checked',
                {'obs_columns': int(len(obj.obs.columns)), 'var_columns': int(len(obj.var.columns))})
        if not valid_ids or not metadata_valid(obj):
            if checkpoint != 'input':
                add('object', 'fail', 'Invalid identifiers or metadata alignment; dependent checks stopped')
            return finish()
        declared_raw = (counts_source.get('kind') == 'raw_counts'
                        and counts_source.get('processing_state') == 'raw'
                        and text(counts_source.get('provenance')) and text(counts_source.get('layer')))
        counts = None
        try:
            counts = layer(obj, counts_source.get('layer'))
            if counts is None or counts.shape != obj.shape:
                raise ValueError('Declared counts has wrong dimensions or is absent')
        except Exception as exc:
            if checkpoint == 'input':
                add('counts_source', 'fail', 'Declared counts layer unavailable: ' + str(exc))
        if checkpoint == 'input' and counts is not None:
            add('counts_source', 'pass' if declared_raw else 'fail',
                'Explicit raw processing state, provenance, and existing counts layer required; numeric appearance is not provenance')
        mito, mito_reason = mito_mask(obj)
        bad, metrics = (None, None)
        if counts is not None:
            bad, metrics = inspect_counts(counts, mito)
            facts['counts_inspection'] = bad
            facts['qc_metrics'] = {key: summary(value) for key, value in metrics.items()}
            facts['mitochondrial_identification'] = mito_reason
        if checkpoint == 'input':
            if bad is not None:
                add('counts_values', 'fail' if any(bad.values()) else 'pass',
                    'Every declared counts value checked for finite, nonnegative, integer-like raw counts', bad)
            conflict = []
            if counts_source.get('layer') == 'X' and any(key in obj.uns for key in ('log1p', 'normalize_total', 'normalization')):
                conflict.append('X declared raw but object records log/normalization processing')
            if re.search(r'(log|norm|scaled)', counts_source.get('layer', ''), flags=re.I):
                conflict.append('Declared raw layer name explicitly denotes processed expression')
            layer_states = obj.uns.get('layer_processing', {})
            if isinstance(layer_states, dict):
                state = layer_states.get(counts_source.get('layer'))
                if state is not None and state != 'raw':
                    conflict.append('Object layer_processing contradicts raw declaration')
            add('processing_state', 'fail' if conflict or not declared_raw else 'pass',
                '; '.join(conflict) if conflict else 'Raw counts declaration checked against available processing metadata; other normalized layers may coexist')
            versions, interfaces = {}, {}
            for package in ('omicverse', 'scanpy', 'anndata', 'numpy', 'scipy', 'pandas'):
                try:
                    versions[package] = importlib.metadata.version(package)
                except importlib.metadata.PackageNotFoundError:
                    versions[package] = None
            interface_names = {'omicverse': ('pp.normalize_total', 'pp.log1p', 'pp.pca', 'pp.neighbors', 'pp.leiden', 'pp.umap'),
                               'scanpy': ('pp.calculate_qc_metrics', 'pp.highly_variable_genes', 'tl.rank_genes_groups')}
            for package, paths in interface_names.items():
                module = sys.modules.get(package)
                for path in paths:
                    target = module
                    for part in path.split('.'):
                        target = getattr(target, part, None) if target is not None else None
                    interfaces[package + '.' + path] = callable(target) if module is not None else None
            facts['environment'] = {'python': platform.python_version(), 'executable': sys.executable,
                                    'packages': versions, 'interfaces': interfaces,
                                    'interface_scope': 'Runtime attributes of loaded packages; lazy package submodule imports may occur, guarded caller RNG restored; no installs'}
            env_status = ('not_checked' if any(v is None for v in versions.values())
                          or any(v is None for v in interfaces.values()) else
                          'fail' if not all(interfaces.values()) else 'pass')
            add('environment', env_status, 'Actual installed package versions and loaded runtime interfaces inspected; omicverse has priority', facts['environment'])
            # Capture diagnostic state without consuming RNG; the public entry
            # guard restores the caller state after any lazy dependency imports.
            import random
            declared_parameters = request.get('parameters') or {}
            seed = declared_parameters.get('seed', declared_parameters.get('random_seed'))
            facts['rng'] = {'declared_seed': seed,
                            'numpy_state_sha256': hashlib.sha256(
                                np.random.get_state()[1].tobytes()
                                + repr((np.random.get_state()[0],) + np.random.get_state()[2:]).encode()).hexdigest(),
                            'python_state_sha256': hashlib.sha256(repr(random.getstate()).encode()).hexdigest()}
            add('seed', 'warning' if isinstance(seed, int) and not isinstance(seed, bool) else 'not_checked',
                'Current RNG states fingerprinted without consumption; the original seed cannot be recovered from state. Declared seed is not measured execution evidence', facts['rng'])
            id_type, species = request.get('gene_id_type'), request.get('species')
            ensembl_fraction = sum(bool(re.match(r'^ENS[A-Z]*G[0-9]+(?:\.[0-9]+)?$', x)) for x in obj.var_names) / obj.n_vars
            id_conflict = ((id_type == 'symbol' and ensembl_fraction > 0.8)
                           or (id_type == 'ensembl' and ensembl_fraction < 0.8))
            add('gene_species', 'fail' if id_conflict else 'not_checked' if mito is None else 'warning',
                'Declared species is not authenticated from symbols; ' + mito_reason,
                {'declared_species': species, 'declared_gene_id_type': id_type,
                 'ensembl_pattern_fraction': ensembl_fraction})
            design = request.get('design') or {}
            objective = design.get('objective') or design.get('research_question')
            repeats = design.get('biological_replicates') or design.get('replicates')
            add('study_design', 'warning' if objective and repeats else 'not_checked',
                'Objective and biological replication are declarations requiring human review; single donor/tutorial cells do not establish treatment/disease replication',
                {'objective_recorded': bool(objective), 'biological_replicates_recorded': bool(repeats)})
            source_files = request.get('source_files') or []
            candidates, source_errors = [], []
            if not isinstance(source_files, list) or len(source_files) > 32:
                add('input_sources', 'not_checked', 'Expected at most 32 declared source files')
                return finish()
            source_layer = details.get('source_counts_layer', counts_source.get('layer'))
            for path in source_files:
                try:
                    info = file_info(path)
                    if str(path).lower().endswith('.h5ad'):
                        original = reopen(path)
                        if (original.obs_names.equals(obj.obs_names) and original.var_names.equals(obj.var_names)
                                and counts is not None and same_matrix(counts, layer(original, source_layer))
                                and same_frame(obj.obs, original.obs, obj.obs.columns)
                                and same_frame(obj.var, original.var, obj.var.columns)):
                            candidates.append((info, original))
                        else:
                            source_errors.append('Source .h5ad does not match full input counts/identifiers/metadata: ' + str(path)[:160])
                    else:
                        source_errors.append('Hashed source format lacks backed correspondence support: ' + str(path)[:160])
                except Exception as exc:
                    source_errors.append(str(exc)[:240])
            if candidates and declared_raw and bad is not None and not any(bad.values()) and not conflict:
                info, original = candidates[0]
                # Rehash after comparison to detect concurrent source modification.
                if file_digest(info['path']) != info['sha256']:
                    raise ValueError('Input source changed during comparison')
                info['counts_layer'] = source_layer
                result['baseline'] = {'schema': 'sandhill-anndata-source-v1', 'source': info,
                                      'input': {'n_obs': int(obj.n_obs), 'n_vars': int(obj.n_vars),
                                                'obs_hash': digest_ids(obj.obs_names), 'var_hash': digest_ids(obj.var_names),
                                                'obs_columns': list(obj.obs.columns), 'var_columns': list(obj.var.columns)}}
                if mito is not None:
                    result['baseline']['mitochondrial_mapping'] = {
                        'gene_ids': list(obj.var_names[mito]), 'source': mito_reason}
                add('input_sources', 'warning' if source_errors else 'pass',
                    'Read-only source full correspondence established; compact source-backed baseline recorded',
                    {'matched_source': info, 'other_source_issues': source_errors[:8]})
            else:
                add('input_sources', 'not_checked',
                    'No trustworthy matching .h5ad source; original subset/count preservation cannot be certified',
                    {'issues': source_errors[:8]})
        elif checkpoint == 'qc':
            if not declared_raw or counts is None or bad is None or any(bad.values()):
                add('qc_metrics', 'fail', 'Raw counts prerequisite failed; QC metrics cannot be trusted', bad)
            else:
                mapping = details.get('qc_metrics') or {}
                checks, unavailable = {}, []
                for metric in ('total_counts', 'n_genes', 'pct_mito'):
                    column = mapping.get(metric)
                    if metric not in metrics:
                        unavailable.append(metric + ': ' + mito_reason)
                    elif not text(column) or column not in obj.obs:
                        unavailable.append(metric + ': saved obs column not supplied/found')
                    else:
                        try:
                            saved = np.asarray(obj.obs[column], dtype=float)
                            recomputed = metrics[metric]
                            checks[metric] = bool(np.isfinite(saved).all() and np.isfinite(recomputed).all()
                                                  and np.allclose(saved, recomputed, rtol=1e-6, atol=1e-6))
                        except Exception:
                            checks[metric] = False
                add('qc_metrics', 'fail' if not all(checks.values()) else 'not_checked' if unavailable else 'pass',
                    'Recomputed all cells from counts and compared saved QC metrics',
                    {'matches': checks, 'unavailable': unavailable})
            thresholds = details.get('qc_thresholds')
            invalid, threshold_results = [], []
            if not isinstance(thresholds, list) or not thresholds:
                add('qc_thresholds', 'not_checked', 'Record data-specific threshold bounds and reasons in qc_thresholds')
            else:
                for entry in thresholds:
                    try:
                        metric = entry['metric']
                        if metric not in ('total_counts', 'n_genes', 'pct_mito') or not text(entry.get('reason')):
                            raise ValueError('Each supported threshold needs a reason')
                        if 'min' not in entry and 'max' not in entry:
                            raise ValueError('Threshold has no bounds')
                        low = number(entry['min']) if entry.get('min') is not None else -math.inf
                        high = number(entry['max']) if entry.get('max') is not None else math.inf
                        if low > high:
                            raise ValueError('Threshold min exceeds max')
                        if metrics is None or metric not in metrics:
                            threshold_results.append({'metric': metric, 'not_checked': True})
                        else:
                            array = metrics[metric]
                            failures = int(np.count_nonzero(~np.isfinite(array) | (array < low) | (array > high)))
                            threshold_results.append({'metric': metric, 'retained_violations': failures})
                    except Exception as exc:
                        invalid.append(str(exc)[:160])
                failures = any(x.get('retained_violations', 0) for x in threshold_results)
                missing = any(x.get('not_checked') for x in threshold_results)
                add('qc_thresholds', 'fail' if invalid or failures else 'not_checked' if missing else 'pass',
                    'Recorded inclusive thresholds and rationale checked against every retained cell; thresholds are not universal biological recommendations',
                    {'checks': threshold_results[:30], 'errors': invalid[:8]})
            try:
                baseline, original = source_from_baseline()
                rows = original.obs_names.get_indexer(obj.obs_names)
                cols = original.var_names.get_indexer(obj.var_names)
                subset = bool((rows >= 0).all() and (cols >= 0).all())
                add('qc_subset', 'pass' if subset else 'fail',
                    'Retained identifiers mapped to original input, independently of gene filtering',
                    {'before': [int(original.n_obs), int(original.n_vars)], 'after': [int(obj.n_obs), int(obj.n_vars)],
                     'cells_reordered': bool(subset and np.any(np.diff(rows) < 0)),
                     'genes_reordered': bool(subset and np.any(np.diff(cols) < 0))})
                if subset:
                    unchanged = (declared_raw and counts is not None and same_matrix(
                        counts, layer(original, baseline['source']['counts_layer']), rows, cols))
                    add('qc_counts_unchanged', 'pass' if unchanged else 'fail',
                        'All retained cell/gene count values compared to original source in bounded blocks; filtering genes does not invalidate comparison')
                    aligned_obs = original.obs.iloc[rows].copy(deep=False)
                    aligned_var = original.var.iloc[cols].copy(deep=False)
                    metadata_ok, metadata_evidence = qc_metadata_matches(
                        aligned_obs, aligned_var, baseline['input']['obs_columns'],
                        baseline['input']['var_columns'])
                    add('qc_metadata', 'pass' if metadata_ok else 'fail',
                        'Original biological metadata preserved in identifier order; fixed QC-derived columns independently recomputed after filtering',
                        metadata_evidence)
                    removed_cells = set(original.obs_names) - set(obj.obs_names)
                    removed_genes = set(original.var_names) - set(obj.var_names)
                    exclusion_path = details.get('qc_exclusions_file')
                    if not removed_cells and not removed_genes and not exclusion_path:
                        add('qc_exclusions', 'pass', 'No cells or genes excluded; full source correspondence verified')
                    elif not exclusion_path:
                        add('qc_exclusions', 'not_checked', 'Per-ID exclusion reasons required for every removed cell/gene',
                            {'removed_cells': len(removed_cells), 'removed_genes': len(removed_genes)})
                    else:
                        evidence, info = read_rows(exclusion_path, ('axis', 'id', 'reason'), allow_empty=True)
                        observed = {'cell': set(), 'gene': set()}
                        errors = 0
                        for row in evidence:
                            axis, identity = row['axis'], str(row['id'])
                            if axis not in observed or not text(row['reason']):
                                errors += 1
                            elif identity in observed[axis]:
                                errors += 1
                            else:
                                observed[axis].add(identity)
                        ok = not errors and observed['cell'] == removed_cells and observed['gene'] == removed_genes
                        add('qc_exclusions', 'pass' if ok else 'fail', 'Exclusion table checked against exact removed identifiers and nonempty reasons',
                            {'file': info, 'removed_cells': len(removed_cells), 'removed_genes': len(removed_genes), 'invalid_rows': errors})
                if file_digest(baseline['source']['path']) != baseline['source']['sha256']:
                    for key in ('qc_subset', 'qc_counts_unchanged', 'qc_metadata', 'qc_exclusions'):
                        add(key, 'not_checked', 'Source changed during QC comparison')
            except Exception as exc:
                add('qc_subset', 'not_checked', 'Original input correspondence unavailable: ' + str(exc))
            doublets = details.get('doublets') or {}
            add('qc_doublets', 'warning' if doublets.get('status') in ('performed', 'not_performed') and text(doublets.get('reason')) else 'not_checked',
                'Doublet assessment/reason is declared, not independently validated; unperformed assessment does not establish doublet exclusion',
                {'declared_status': str(doublets.get('status', ''))[:80], 'reason_recorded': text(doublets.get('reason'))})
        else:
            saved = None
            try:
                info = file_info(details.get('result_file'))
                saved = reopen(info['path'])
                ids_equal = saved.obs_names.equals(obj.obs_names) and saved.var_names.equals(obj.var_names)
                matrices_equal = (ids_equal and same_matrix(obj.X, saved.X)
                                  if obj.X is not None and saved.X is not None
                                  else obj.X is None and saved.X is None)
                if counts is not None and counts_source.get('layer') != 'X':
                    matrices_equal = matrices_equal and same_matrix(counts, layer(saved, counts_source['layer']))
                expression_name = details.get('expression_layer')
                if text(expression_name) and expression_name not in ('X', counts_source.get('layer')):
                    matrices_equal = matrices_equal and same_matrix(
                        layer(obj, expression_name), layer(saved, expression_name))
                if obj.X is not None:
                    matrices_equal = matrices_equal and all(np.isfinite(values(block)).all()
                                                            for _, block in blocks(obj.X))
                matrices_equal = matrices_equal and declared_raw and bad is not None and not any(bad.values())
                if file_digest(info['path']) != info['sha256']:
                    raise ValueError('Result file changed during comparison')
                add('result_file', 'pass' if ids_equal and matrices_equal else 'fail',
                    'Actual .h5ad reopened read-only; live/saved identifiers, X and declared counts compared',
                    {'file': info, 'dimensions': [int(saved.n_obs), int(saved.n_vars)]})
                columns_equal = (obj.obs.columns.equals(saved.obs.columns) and obj.var.columns.equals(saved.var.columns))
                frames_equal = (columns_equal and same_frame(obj.obs, saved.obs, obj.obs.columns)
                                and same_frame(obj.var, saved.var, obj.var.columns))
                add('result_metadata', 'pass' if frames_equal else 'fail',
                    'Every live/saved obs and var metadata column, dtype, category and identifier alignment compared')
            except Exception as exc:
                add('result_file', 'not_checked' if isinstance(exc, EvidenceUnavailable) else 'fail',
                    'Saved result cannot be verified: ' + str(exc))
            cluster_key, embedding_key = details.get('cluster_key'), details.get('embedding_key')
            labels = None
            if not text(cluster_key) or cluster_key not in obj.obs:
                add('result_clusters', 'not_checked', 'Explicit cluster_key must identify a live obs column')
            else:
                cluster_values = obj.obs[cluster_key]
                labels = np.asarray(cluster_values.astype(str))
                valid = not cluster_values.isna().any() and all(text(x) for x in labels)
                match = saved is not None and cluster_key in saved.obs and cluster_values.equals(saved.obs[cluster_key])
                add('result_clusters', 'pass' if valid and match else 'fail',
                    'Live nonmissing clusters compared to reopened result',
                    {'cluster_key': cluster_key, 'n_clusters': int(len(set(labels)))})
            try:
                if not text(embedding_key) or embedding_key not in obj.obsm:
                    raise ValueError('Explicit embedding_key is absent')
                keys = list(obj.obsm.keys())
                embedding_errors = []
                for key in keys:
                    left = obj.obsm[key]
                    if not hasattr(left, 'shape') or left.shape[0] != obj.n_obs:
                        embedding_errors.append(str(key) + ': dimensions')
                        continue
                    if getattr(left, 'ndim', 2) != 2 or len(left.shape) != 2 or left.shape[1] == 0:
                        embedding_errors.append(str(key) + ': not a nonempty 2D embedding')
                        continue
                    for _, block in blocks(left):
                        if not np.isfinite(values(block)).all():
                            embedding_errors.append(str(key) + ': nonfinite')
                            break
                    if saved is None or key not in saved.obsm or not same_matrix(left, saved.obsm[key]):
                        embedding_errors.append(str(key) + ': saved mismatch')
                if saved is not None and set(saved.obsm.keys()) != set(keys):
                    embedding_errors.append('Live/saved embedding key sets differ')
                add('result_embeddings', 'fail' if embedding_errors else 'pass',
                    'All live embeddings fully inspected and compared to reopened object',
                    {'embedding_key': embedding_key, 'n_embeddings': len(keys), 'issues': embedding_errors[:16]})
            except Exception as exc:
                add('result_embeddings', 'not_checked', str(exc))
            evidence = details.get('parameter_evidence') or {}
            missing, mismatch, observed = [], [], {}
            minimum_keys = {'pca': ('n_pcs', 'n_comps', 'zero_center'),
                            'neighbors': ('n_neighbors',), 'clustering': ('resolution',),
                            'umap': ('min_dist', 'a')}
            for step in ('pca', 'neighbors', 'clustering', 'umap'):
                record = evidence.get(step)
                if not isinstance(record, dict) or not isinstance(record.get('parameters'), dict) or not record['parameters']:
                    missing.append(step + ': explicit parameter evidence absent')
                    continue
                key = record.get('uns_key')
                actual = obj.uns.get(key, {}) if text(key) else {}
                actual = actual.get('params') if isinstance(actual, dict) else None
                if not isinstance(actual, dict):
                    missing.append(step + ': actual uns params absent')
                    continue
                observed[step] = {'uns_key': key, 'parameter_keys': sorted(str(k) for k in actual)[:50]}
                if not any(param in record['parameters'] for param in minimum_keys[step]):
                    missing.append(step + ': key algorithm parameters not recorded')
                unmeasured = []
                for param, value in record['parameters'].items():
                    if param not in actual:
                        unmeasured.append(str(param))
                        continue
                    try:
                        equal = bool(np.array_equal(np.asarray(actual[param]), np.asarray(value)))
                    except Exception:
                        equal = False
                    if not equal:
                        mismatch.append(step + ': ' + str(param))
                seed = record.get('seed')
                runtime_seed = actual.get('random_state', actual.get('seed', actual.get('random_seed')))
                if not isinstance(seed, int) or isinstance(seed, bool):
                    missing.append(step + ': explicit recorded seed unavailable')
                elif runtime_seed is not None and runtime_seed != seed:
                    mismatch.append(step + ': seed')
                observed[step]['declared_only_parameters'] = unmeasured[:30]
                observed[step]['seed_evidence'] = ('object runtime metadata' if runtime_seed is not None
                                                  else 'recorded declaration with actual executed source; RNG not independently measured')
                if saved is not None:
                    disk_actual = saved.uns.get(key, {})
                    disk_actual = disk_actual.get('params') if isinstance(disk_actual, dict) else None
                    if not isinstance(disk_actual, dict):
                        mismatch.append(step + ': saved runtime parameters absent')
                    else:
                        for param in actual:
                            if param not in disk_actual or not np.array_equal(np.asarray(actual[param]), np.asarray(disk_actual[param])):
                                mismatch.append(step + ': saved ' + str(param))
            code_files = details.get('code_files')
            code_facts = []
            if not isinstance(code_files, list) or not code_files or len(code_files) > 32:
                missing.append('Code document paths missing')
            else:
                for path in code_files:
                    try:
                        code_facts.append(file_info(path))
                    except Exception as exc:
                        missing.append('Code document: ' + str(exc)[:120])
            executions = request.get('execution_evidence')
            if not isinstance(executions, list) or not executions or not all(
                    isinstance(run, dict) and run.get('status') == 'ok'
                    and text(run.get('code')) and run.get('run_id') is not None
                    for run in executions):
                missing.append('Daemon-verified successful same-session execution trace missing')
            else:
                facts['execution_trace'] = [{'run_id': run['run_id'],
                                             'code_digest': run.get('code_digest'),
                                             'seed_scope': 'recorded parameters plus executed source, not hidden RNG proof'}
                                            for run in executions[:32]]
            facts['code_files'] = code_facts
            add('result_parameters', 'fail' if mismatch else 'not_checked' if missing else 'pass',
                'Recorded parameters/seeds checked for completeness and consistency with emitted runtime metadata and actual daemon-journal execution trace. Non-emitted values remain declarations, not independently measured RNG or hidden library arguments',
                {'observed': observed, 'missing': missing[:24], 'mismatches': mismatch[:24]})
            expression_name = details.get('expression_layer')
            expression = None
            try:
                expression = layer(obj, expression_name)
                if expression is None or expression.shape != obj.shape:
                    raise ValueError('Expression matrix absent or wrong shape')
            except Exception:
                expression = None
            marker_rows, annotation_rows = None, None
            marker_info, annotation_info = None, None
            if labels is not None and expression is not None:
                valid_clusters = set(labels)
                try:
                    marker_rows, marker_info = read_rows(details.get('marker_file'),
                                                       ('cluster', 'gene', 'p_val_adj', 'pct_expr', 'mean_expr'))
                    marker_pairs = [(str(r['cluster']), str(r['gene'])) for r in marker_rows]
                    if len(set(marker_pairs)) != len(marker_pairs):
                        raise ValueError('Duplicate cluster/gene marker rows')
                    if any(c not in valid_clusters for c, _ in marker_pairs):
                        raise ValueError('Marker cluster is not present in live object')
                    if set(c for c, _ in marker_pairs) != valid_clusters:
                        raise ValueError('Marker evidence does not cover every cluster')
                    if any(not 0 <= number(row['p_val_adj']) <= 1 for row in marker_rows):
                        raise ValueError('Adjusted p-values must be finite and in [0,1]')
                    if not text(details.get('marker_method')) or details.get('p_adjust_method') not in ('benjamini-hochberg', 'bonferroni', 'BH', 'fdr_bh'):
                        raise ValueError('Explicit marker method and supported multiple-testing correction required')
                except Exception as exc:
                    marker_rows = None
                    add('result_markers', 'not_checked' if isinstance(exc, (EvidenceUnavailable, OSError, TypeError)) else 'fail', str(exc))
                try:
                    annotation_rows, annotation_info = read_rows(details.get('annotation_file'),
                        ('cluster', 'label', 'gene', 'pct_expr', 'mean_expr', 'evidence_type', 'source', 'status', 'reason'), True)
                    coverage = {}
                    for row in annotation_rows:
                        cluster = str(row['cluster'])
                        if cluster not in valid_clusters:
                            raise ValueError('Annotation cluster absent from live object')
                        if not text(row['label']) or not text(row['source']) or not text(row['reason']):
                            raise ValueError('Every annotation row needs label, source and evidence/conflict reason')
                        if row['status'] not in ('confirmed', 'pending', 'unknown') or row['evidence_type'] not in ('support', 'conflict'):
                            raise ValueError('Invalid annotation status/evidence_type')
                        state = coverage.setdefault(cluster, {'support': set(), 'conflict': False, 'labels': set(), 'unknown': False})
                        state['labels'].add(str(row['label']))
                        state['unknown'] |= row['status'] in ('pending', 'unknown') or str(row['label']).lower() == 'unknown'
                        if row['evidence_type'] == 'support':
                            state['support'].add(str(row['gene']))
                        else:
                            state['conflict'] = True
                    if set(coverage) != valid_clusters:
                        raise ValueError('Annotation table must cover every live cluster')
                    if any(len(v['support']) < 2 for v in coverage.values()):
                        raise ValueError('At least two distinct support markers required per cluster; two names alone do not establish biological correctness')
                    if any(len(v['labels']) != 1 for v in coverage.values()):
                        raise ValueError('Conflicting candidate labels within a cluster require reconciliation')
                    for cluster, state in coverage.items():
                        reasons = ' '.join(str(r['reason']).lower() for r in annotation_rows
                                           if str(r['cluster']) == cluster)
                        if not state['conflict'] and not re.search(r'no[- ]conflict|no conflicting|conflict.{0,30}(none|absent|not found)', reasons):
                            raise ValueError('Every cluster needs conflict evidence or an explicit assessed-no-conflict reason')
                    tissue = str((request.get('design') or {}).get('tissue', '')).strip().lower()
                    supported_context = request.get('species') == 'human' and tissue in (
                        'pbmc', 'blood', 'peripheral blood')
                    if not supported_context and any(not v['unknown'] for v in coverage.values()):
                        raise ValueError('Confident PBMC labels require human blood context; unsupported species/tissue must retain unknown/pending annotation')
                except Exception as exc:
                    annotation_rows = None
                    add('result_annotation', 'not_checked' if isinstance(exc, (EvidenceUnavailable, OSError, TypeError)) else 'fail', str(exc))
                try:
                    combined = (marker_rows or []) + (annotation_rows or [])
                    if combined:
                        pairs = set((str(row['cluster']), str(row['gene'])) for row in combined)
                        measured = expression_summaries(expression, labels, pairs)
                        if marker_rows is not None:
                            errors = match_summaries(marker_rows, measured)
                            # Verify exported adjusted p-values against the actual
                            # rank_genes_groups output, not just a plausible column.
                            rank_key = details.get('marker_uns_key', 'rank_genes_groups')
                            rank = obj.uns.get(rank_key)
                            pvalue_errors, pvalue_missing = 0, False
                            disk_rank = saved.uns.get(rank_key) if saved is not None else None
                            if not isinstance(disk_rank, dict):
                                pvalue_errors += 1
                            elif isinstance(rank, dict):
                                live_params, disk_params = rank.get('params'), disk_rank.get('params')
                                if (not isinstance(live_params, dict) or not isinstance(disk_params, dict)
                                        or set(live_params) != set(disk_params)):
                                    pvalue_errors += 1
                                else:
                                    for key, value in live_params.items():
                                        if not np.array_equal(np.asarray(value), np.asarray(disk_params[key])):
                                            pvalue_errors += 1
                                # Compare the entire recorded marker family, not
                                # only genes selected for an exported CSV.
                                for statistic in ('names', 'pvals', 'pvals_adj', 'scores', 'logfoldchanges'):
                                    if statistic not in rank and statistic not in disk_rank:
                                        continue
                                    if statistic not in rank or statistic not in disk_rank:
                                        pvalue_errors += 1
                                        continue
                                    left, right = rank[statistic], disk_rank[statistic]
                                    if (getattr(left.dtype, 'names', None) != getattr(right.dtype, 'names', None)
                                            or left.shape != right.shape):
                                        pvalue_errors += 1
                                        continue
                                    for group in left.dtype.names or ():
                                        a, b = np.asarray(left[group]), np.asarray(right[group])
                                        equal = (np.array_equal(a, b, equal_nan=True)
                                                 if np.issubdtype(a.dtype, np.number)
                                                 else np.array_equal(a.astype(str), b.astype(str)))
                                        if not equal:
                                            pvalue_errors += 1
                            if not isinstance(rank, dict) or not all(k in rank for k in ('names', 'pvals_adj', 'params')):
                                pvalue_missing = True
                            else:
                                params = rank['params']
                                actual_layer = params.get('layer')
                                actual_use_raw = params.get('use_raw')
                                if actual_use_raw is None:
                                    pvalue_missing = True
                                expected_layer = None if expression_name == 'X' else expression_name
                                if actual_use_raw or actual_layer != expected_layer:
                                    pvalue_errors += 1
                                if params.get('groupby') != cluster_key or params.get('method') != details.get('marker_method'):
                                    pvalue_errors += 1
                                correction = {'BH': 'benjamini-hochberg', 'fdr_bh': 'benjamini-hochberg'}.get(details.get('p_adjust_method'), details.get('p_adjust_method'))
                                if params.get('corr_method') is not None and params.get('corr_method') != correction:
                                    pvalue_errors += 1
                                lookup, raw_lookup = {}, {}
                                for cluster in set(str(r['cluster']) for r in marker_rows):
                                    try:
                                        names = rank['names'][cluster]
                                        adjusted = np.asarray(rank['pvals_adj'][cluster], dtype=float)
                                        raw = np.asarray(rank['pvals'][cluster], dtype=float)
                                        lookup[cluster] = dict(zip(map(str, names), adjusted))
                                        raw_lookup[cluster] = dict(zip(map(str, names), raw))
                                        if (len(names) != len(raw) or len(names) != len(adjusted)
                                                or raw.ndim != 1 or adjusted.ndim != 1):
                                            pvalue_errors += 1
                                            continue
                                        named_genes = set(map(str, names))
                                        if len(named_genes) != len(names) or not named_genes.issubset(set(obj.var_names)):
                                            pvalue_errors += 1
                                            continue
                                        if len(names) != obj.n_vars:
                                            pvalue_missing = True
                                            continue
                                        if named_genes != set(obj.var_names):
                                            pvalue_errors += 1
                                            continue
                                        if (not np.isfinite(raw).all() or not np.isfinite(adjusted).all()
                                                or (raw < 0).any() or (raw > 1).any()):
                                            pvalue_errors += 1
                                            continue
                                        if correction == 'bonferroni':
                                            recomputed = np.minimum(1.0, raw * obj.n_vars)
                                        else:
                                            order = np.argsort(raw)
                                            ranked = raw[order] * len(raw) / np.arange(1, len(raw) + 1)
                                            corrected = np.minimum(1.0, np.minimum.accumulate(ranked[::-1])[::-1])
                                            recomputed = np.empty_like(corrected)
                                            recomputed[order] = corrected
                                        if not np.allclose(adjusted, recomputed, rtol=1e-6, atol=1e-12):
                                            pvalue_errors += 1
                                    except (KeyError, IndexError, TypeError, ValueError):
                                        pvalue_missing = True
                                for row in marker_rows:
                                    cluster, gene = str(row['cluster']), str(row['gene'])
                                    p = lookup.get(cluster, {}).get(gene)
                                    if p is None:
                                        pvalue_missing = True
                                    elif not math.isfinite(p) or not math.isclose(p, number(row['p_val_adj']), rel_tol=1e-6, abs_tol=1e-12):
                                        pvalue_errors += 1
                                    if 'p_val' in row:
                                        raw_p = raw_lookup.get(cluster, {}).get(gene)
                                        if raw_p is None:
                                            pvalue_missing = True
                                        elif not math.isclose(raw_p, number(row['p_val']), rel_tol=1e-6, abs_tol=1e-12):
                                            pvalue_errors += 1
                            add('result_markers', 'fail' if errors or pvalue_errors else 'not_checked' if pvalue_missing else 'pass',
                                'Expression summaries independently recomputed; complete live/saved marker parameters and statistics compared, and full-family adjusted p-values recomputed. Post-clustering p-values remain exploratory',
                                {'file': marker_info, 'rows': len(marker_rows), 'expression_layer': expression_name,
                                 'summary_mismatches': errors, 'recorded_marker_mismatches': pvalue_errors,
                                 'missing_recorded_marker_evidence': pvalue_missing})
                        if annotation_rows is not None:
                            errors = match_summaries(annotation_rows, measured)
                            add('result_annotation', 'fail' if errors else 'warning',
                                'Multi-marker evidence recomputed; sources/conflict reasons checked for presence only, not authenticated. Biological identity and human assertions remain under independent review',
                                {'file': annotation_info, 'rows': len(annotation_rows), 'clusters': len(coverage),
                                 'summary_mismatches': errors, 'expression_layer': expression_name})
                except Exception as exc:
                    for key in ('result_markers', 'result_annotation'):
                        if key not in items:
                            add(key, 'fail', 'Expression evidence could not be independently recomputed: ' + str(exc))
            else:
                add('result_markers', 'not_checked', 'Valid cluster labels and explicit existing expression_layer are required')
                add('result_annotation', 'not_checked', 'Valid cluster labels and explicit existing expression_layer are required')
            design = request.get('design') or {}
            forbidden = any(design.get(key) for key in ('disease_comparison', 'treatment_comparison', 'group_differential_expression'))
            add('result_design', 'fail' if forbidden else 'warning',
                'Treatment/disease differential inference is outside this exploratory workflow; no scientific/statistical design approval is provided')
            add('human_review', 'warning', 'Independent human review of design, QC tradeoffs, annotation conflicts, sources and conclusions is always required; technical checks do not prove scientific correctness')
            if saved is not None:
                try:
                    if file_digest(info['path']) != info['sha256']:
                        for key in ('result_file', 'result_metadata', 'result_clusters', 'result_embeddings', 'result_parameters'):
                            add(key, 'not_checked', 'Saved result changed during full inspection')
                except OSError as exc:
                    add('result_file', 'not_checked', 'Saved result became unavailable during inspection: ' + str(exc))
    except Exception as exc:
        add('checker_execution', 'not_checked', 'Inspection could not complete; no sampled pass: ' + type(exc).__name__ + ': ' + str(exc))
    return finish()


def sandhill_bio_check(request, namespace):
    """Preserve caller RNG state across dependency imports and read-only IO."""
    import random
    import sys

    python_state = random.getstate()
    numpy_module = sys.modules.get('numpy')
    numpy_state = numpy_module.random.get_state() if numpy_module is not None else None
    try:
        return _sandhill_bio_check_impl(request, namespace)
    finally:
        # Dependency lazy imports/read-only deserializers may consume global
        # RNG state. This guard covers success, early returns and exceptions.
        random.setstate(python_state)
        if numpy_state is not None:
            numpy_module.random.set_state(numpy_state)
