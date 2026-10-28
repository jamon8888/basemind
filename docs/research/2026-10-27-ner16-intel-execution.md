# Exécution NER sur machines récentes de 16 Go : pistes et banc Intel contraint

Recherche du 27 octobre 2026, relue le 28 octobre, pour [Recherche : quelles accélérations pour les machines récentes 16 Go et le banc Intel contraint ?](https://github.com/jamon8888/basemind/issues/120), dans la [Carte wayfinder : performance de l’anonymisation sur 16 Go, avec banc matériel contraint](https://github.com/jamon8888/basemind/issues/118).

## Périmètre et niveau de preuve

**La cible produit est une machine récente de 16 Go, pas le Mac de 2012.** L’i7-3520M disponible (2 cœurs physiques, 4 logiques, AVX1/F16C sans AVX2, macOS 14.7.4) est un banc contraint. Les objectifs p95 à chaud, moins d’une seconde pour 500 caractères et moins de trois secondes pour 2 000 caractères, devront être qualifiés sur les familles de machines récentes retenues. Une réussite ancienne est encourageante ; un échec ancien n’élimine pas une solution récente.

Cette note est une inspection, sans compilation, conversion ni benchmark. Sources locales : basemind `c6a8c55a67a431ad14cb0abbdeaa292f135555ce`, dépendances du `Cargo.lock` effectivement inspectées dans le registre Cargo, xberg/xberg-gliner 1.3.9 et Candle 0.11.0. Les SHA amont des sources publiées viennent de leurs `.cargo_vcs_info.json`. Les chiffres de l’audit fourni concernent un autre ordinateur, i5-7360U/8 Go, et une comparaison Python ; ils ne prouvent aucune accélération du binaire actuel.

## Faits du chemin actuel

Le masquage appelle `CandleBackend::get_or_init`, puis `detect` successivement pour chaque fenêtre. Le modèle reste dans un cache global par couple **chemin du modèle/chemin d’adaptateur**, avec un mutex autour de chaque inférence. Ce cache ne mémorise pas les résultats des messages et n’est pas partagé entre processus. Changer la représentation d’un chemin peut créer une autre entrée ; multiplier les processus peut multiplier les poids. L’attente du verrou, le chargement, le fenêtrage et le calcul doivent donc être chronométrés séparément. [Appel basemind](https://github.com/jamon8888/basemind/blob/c6a8c55a67a431ad14cb0abbdeaa292f135555ce/src/mcp/tools_redact.rs#L713), [cache et verrou xberg](https://github.com/xberg-io/xberg/blob/fdb16150ed0bf8578d68e20ec32301bc2caa8fd1/crates/xberg/src/text/ner/candle.rs).

Le chargement local choisit CPU et F32 ; l’encodeur est DeBERTa-v2/v3. Le pipeline reçoit un seul couple texte/catégories et construit des tenseurs de dimension batch=1. Une boucle de plusieurs fenêtres n’est donc pas un lot vectorisé. La présence de fonctionnalités Metal/CUDA dans la bibliothèque ne signifie pas que cet appel basemind les utilise. [Modèle](https://github.com/xberg-io/xberg/blob/fdb16150ed0bf8578d68e20ec32301bc2caa8fd1/crates/xberg-gliner/src/candle/model.rs), [pipeline](https://github.com/xberg-io/xberg/blob/fdb16150ed0bf8578d68e20ec32301bc2caa8fd1/crates/xberg-gliner/src/candle/pipeline.rs).

Deux réglages de fils existent dans Candle 0.11 : `RAYON_NUM_THREADS` gouverne le parallélisme GEMM et le pool Rayon Candle ; `CANDLE_NUM_THREADS` gouverne son autre pool, `BarrierPool`. Le défaut utilise les cœurs physiques, ou les cœurs performance sur Apple Silicon. `OMP_NUM_THREADS` n’est pas le réglage du GEMM Rust observé. Tester chaque combinaison dans un processus neuf : les pools sont initialisés une fois. Les limites `embed_threads` de basemind concernent les embeddings, pas automatiquement ce NER Candle. [Pools Candle](https://github.com/huggingface/candle/blob/31f35b147389700ed2a178ee66a91c3cc25cc80d/candle-core/src/utils.rs#L302), [GEMM](https://github.com/huggingface/candle/blob/31f35b147389700ed2a178ee66a91c3cc25cc80d/candle-core/src/cpu_backend/mod.rs#L1372).

**Piste de compilation concrète :** le profil release de basemind impose `opt-level="s"` à toutes les dépendances, sauf exceptions qui n’incluent ni Candle ni GEMM. Une expérience ciblée `opt-level=3` sur la chaîne NER peut aider ; ce n’est pas un gain établi. Conserver la contrainte documentée de taille du binaire ARM64 et vérifier le packaging après mesure. Ne pas généraliser `target-cpu=native` aux binaires distribués. [Profil Cargo](https://github.com/jamon8888/basemind/blob/c6a8c55a67a431ad14cb0abbdeaa292f135555ce/Cargo.toml).

## Options : garder ouvertes les machines récentes

| Option | Décision de recherche |
|---|---|
| Candle F32, threads et compilation ciblée | Première référence comparable sur toutes les machines ; mesurer sans changer le modèle. Sur l’ancien Intel essayer 1/2/4 fils, sans supposer que quatre gagnent. |
| Candle avec Accelerate | Expérience macOS distincte : Candle possède un chemin BLAS Accelerate, absent des dépendances verrouillées observées. Nécessite intégration et validation, pas seulement une variable d’environnement. |
| ONNX CPU F32 puis int8 | Candidat sérieux sur x86 récent ; export du modèle précis et parité préalables. Conserver aussi une mesure sur l’ancien Intel, sans en faire un veto. |
| Apple Silicon : Candle Metal ou ONNX/CoreML | Candidats dédiés, à comparer au CPU sur une machine réelle de 16 Go. Vérifier opérations supportées, transferts et compilation initiale. |
| Lots de fenêtres | Prototype ultérieur, surtout pour documents : petites tailles homogènes, mémoire et p95 messages surveillés. Pas de réglage batch prêt à activer dans le chemin actuel. |

**ONNX n’est pas un remplacement de fichier interchangeable.** xberg-gliner possède bien un moteur GLiNER2 ONNX distinct du GLiNER classique : cinq entrées (`input_ids`, `attention_mask`, `text_positions`, `schema_positions`, `span_idx`), sortie `span_scores`, un texte par appel. Le GLiNER classique utilise un autre schéma et `logits`. Vérifier l’export exact de `fastino/gliner2-privacy-filter-PII-multi`, ses têtes de comptage/scoring, le tokenizer, les offsets UTF-8, les catégories et les seuils. L’existence du moteur ne prouve ni un export compatible de ce modèle ni la parité du décodage. [Moteur v2](https://github.com/xberg-io/xberg/blob/fdb16150ed0bf8578d68e20ec32301bc2caa8fd1/crates/xberg-gliner/src/v2/engine.rs), [schéma v2](https://github.com/xberg-io/xberg/blob/fdb16150ed0bf8578d68e20ec32301bc2caa8fd1/crates/xberg-gliner/src/v2/session.rs).

Quantifier seulement l’encodeur et garder les têtes en F32 est une **hypothèse d’implémentation**, pas une option existante du chargeur Candle. ORT recommande notamment d’étudier la quantification dynamique des transformers ; le résultat dépend des noyaux matériels, avec des particularités de saturation sur AVX2/AVX512. Réduction de poids ne signifie pas réduction identique du temps ni du processus complet. Mesurer rappel par catégorie et écarts de scores, pas seulement taille du modèle. [Quantification ORT](https://onnxruntime.ai/docs/performance/model-optimizations/quantization.html).

F16C ne justifie pas à lui seul une accélération native FP16 ; le chemin Accelerate Candle inspecté refuse même le matmul F16. Sur Apple Silicon, un chemin GPU est une expérience différente. CoreML propose CPU/GPU/ANE selon compatibilité, mais formes dynamiques et compilation peuvent coûter cher ; inspecter l’affectation réelle des opérateurs et son cache compilé. Les 16 Go peuvent également servir au GPU : ne pas compter cette mémoire deux fois comme une réserve séparée. [Candle Accelerate](https://github.com/huggingface/candle/blob/31f35b147389700ed2a178ee66a91c3cc25cc80d/candle-core/src/cpu_backend/mod.rs#L1455), [CoreML ORT](https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html).

## Mémoire et chantier global

Les réglages [Optimisation des défauts pour 8 Go](https://github.com/jamon8888/basemind/pull/23) sont déjà intégrés : scan=2, embeddings=2, un document simultané, lot d’embeddings=16, cache de carte=128 Mio. Ce sont des plafonds à conserver comme référence ; doubler la RAM ne justifie pas de doubler chaque parallélisme. Le code désactive aussi l’arène CPU et les memory patterns ORT, et libère les moteurs d’embeddings après les passes. Ce comportement ne libère pas automatiquement le cache Candle. [Embeddings](https://github.com/jamon8888/basemind/blob/c6a8c55a67a431ad14cb0abbdeaa292f135555ce/src/embeddings.rs), [ressources](https://github.com/jamon8888/basemind/blob/c6a8c55a67a431ad14cb0abbdeaa292f135555ce/src/config/resources.rs).

Mesurer poids résidents, activations, buffers, empreinte de chaque processus, compression et swap pendant OCR + embeddings + anonymisation. Le [chantier GraphRAG](https://github.com/jamon8888/basemind/issues/98) ajoute du travail de graphe ; la [passerelle gaze](https://github.com/jamon8888/basemind/issues/106) conserve une frontière de processus. Un cache global basemind ne mutualise donc pas spontanément les modèles de la passerelle. Proposer une priorité messages entre fenêtres, en laissant finir l’inférence courante ; retirer le mutex ou cloner le modèle sans preuve serait prématuré. La préanalyse pendant la frappe doit annuler les demandes devenues inutiles et vérifier la version exacte à l’envoi.

Les [défauts xberg d’OCR et masquage](https://github.com/jamon8888/basemind/issues/113) constituent un chantier de **correction**, pas des optimisations : chevauchement téléphone/adresse, filtre dictionnaire supprimant du texte, alpha d’image mal composé. Une extraction qui perd 93 % du texte peut paraître très rapide ; corriger l’OCR augmente légitimement le travail NER. Comparer à couverture égale et garder des essais texte pur séparés. Le [signalement historique d’extraction](https://github.com/jamon8888/basemind/issues/95) doit se lire avec ces diagnostics plus récents.

Enfin, l’[incident ONNX Intel](https://github.com/jamon8888/basemind/issues/17) ne décrit plus exactement le packaging présent : celui-ci épingle déjà ORT 1.23.2 avec checksum. Tout nouvel export doit fonctionner avec le runtime réellement livré et son ABI/opset ; un résultat Python récent ne suffit pas. Cela ne contraint pas les autres plateformes à ce runtime Intel. [Packaging actuel](https://github.com/jamon8888/basemind/blob/c6a8c55a67a431ad14cb0abbdeaa292f135555ce/scripts/package-release.sh#L160).

## Ordre des expériences

1. Référence reproductible : binaire, modèles et politiques identifiés ; temps à froid/chaud, file d’attente, inférence et mémoire séparés ; texte complet vérifié.
2. Profils de catégories/fenêtres étudiés ailleurs dans la carte, puis réglages Candle et compilation ciblée, un facteur à la fois.
3. Priorité messages face aux documents/OCR/embeddings ; préanalyse pendant frappe avec coût CPU au repos et sous saisie.
4. Parité ONNX F32 du modèle exact, puis int8 sur CPU récent ; Metal/CoreML sur Apple Silicon si cette famille est retenue.
5. Lots, duplication contrôlée ou modèle plus petit seulement si les mesures précédentes en justifient le coût.

Confiance élevée sur le code inspecté ; incertitude sur les gains, l’export quantifié et les cibles matérielles exactes. Aucun SLO n’est démontré par cette recherche.
