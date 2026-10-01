# Coucou — intégrations

Règle d'or : **vérifier la doc officielle au moment d'implémenter**. Les formats ci-dessous sont le plan, pas une garantie. Sources à relire :
- Hooks Claude Code : https://code.claude.com/docs/en/hooks
- API Claude (Messages, outil de recherche web, modèles) : https://docs.claude.com/en/api/overview
- API publique n8n : `{URL de l'instance}/api/v1/docs` (playground de l'instance de Louis)

---

## 1. Claude Code (sessions de Louis)

### Architecture
```
claude (terminal, VS Code, app Claude)
  └─ hook "command" ─► coucou-hook (petit exécutable Rust, livré avec l'app)
                         └─ named pipe (Windows) / socket Unix (Linux) ─► Coucou
                         ◄─ décision (pour PermissionRequest)
```
- `coucou-hook` : crate `windows/hook`, copié au lancement dans `%LOCALAPPDATA%\Coucou\bin\coucou-hook.exe` (Windows) ou `~/.local/share/coucou/bin/coucou-hook` (Linux).
- Canal : named pipe `\\.\pipe\coucou-<SID>` sous Windows (le serveur est vérifié comme appartenant au même utilisateur) ; socket `/run/user/<uid>/coucou/hook.sock` sous Linux, dossier en 0700, socket en 0600, utilisateur vérifié des deux côtés avec `SO_PEERCRED`. 1 Mio maximum par message.
- `coucou-hook <Event>` lit le JSON du hook sur stdin, ajoute le contexte du terminal (`TERM_PROGRAM`, `WT_SESSION`, `TERM_SESSION_ID`, `VSCODE_PID`, `cwd`), l'envoie à l'app.
- **Si l'app ne répond pas en 300 ms, `coucou-hook` sort en code 0 sans rien écrire** : Claude Code continue normalement. Jamais de blocage.

### Événements à brancher et état du bonhomme
| Hook | Effet dans l'app |
|---|---|
| `SessionStart` | crée la tâche (nom = dossier), état `idle` |
| `UserPromptSubmit` | état `thinking`, ligne du défilé = début du prompt |
| `PreToolUse` | état `working`, ligne = outil + cible (« Edit invoice.ts », « Bash npm test ») |
| `PostToolUse` / `PostToolUseFailure` | met à jour la ligne ; un échec reste `working` |
| `PermissionRequest` | alerte `approval` (voir plus bas) |
| `Notification` | selon le type : attente d'entrée → `question` si une question est posée, sinon rien ; limite d'usage → `ratelimit` |
| `Stop` | état `finished` → vue `finished` 5,2 s, résumé = dernière phrase utile de la réponse si disponible |
| `StopFailure` (si présent dans la doc) | alerte `error` |
| `SubagentStart` / `SubagentStop` | afficher « + sous-agent » dans le défilé |
| `SessionEnd` | retire la tâche |

Vérifier dans la doc la liste exacte des événements et leurs champs.

### Approuver depuis l'island
- Sur `PermissionRequest`, `coucou-hook` **attend** la décision de l'app (défaut 110 s, réglable) puis écrit sur stdout le JSON de décision du hook (d'après la doc actuelle : `hookSpecificOutput` avec `decision.behavior` = `allow` ou `deny`). Timeout du hook dans settings.json : décision + 10 s.
- Pas de réponse avant le délai, ou app fermée → aucune sortie, le terminal affiche sa demande habituelle. Si Louis répond dans le terminal, l'app retire l'alerte au prochain événement de la session.
- Un bug a été signalé où `deny` était ignoré sur `PermissionRequest` (issue GitHub anthropics/claude-code #19298). **Tester allow et deny** ; si deny ne marche pas, basculer la décision sur `PreToolUse` (`permissionDecision`) pour les outils concernés.
- « Toujours autoriser » : si la doc permet de renvoyer une règle de permission persistante, l'utiliser. Sinon l'app garde sa propre liste (projet + outil + motif de commande) et répond `allow` automatiquement ensuite. Liste visible et supprimable dans les réglages.
- Raccourcis Y / N quand la vue `approval` est ouverte.

### Répondre aux questions
- Si Claude utilise l'outil de question (`AskUserQuestion`), l'intercepter en `PreToolUse` et afficher les options dans la vue `question`.
- Vérifier dans la doc si un hook peut fournir la réponse. Si oui : clic sur une option = réponse. **Si non** : la vue affiche la question et un bouton « Répondre dans le terminal » qui saute à la session. Ne pas bricoler de frappe clavier simulée.

### Ouvrir le terminal
Le bouton « Ouvrir le terminal » ouvre le dossier `cwd` de la session dans le premier éditeur trouvé dans le `PATH` (`code`, puis sous Linux `code-oss`, `codium`, `cursor`), sinon dans le gestionnaire de fichiers. Pas de saut vers un onglet de terminal précis dans cette version.

### Installation des hooks : procédure obligatoire
1. Lire `~/.claude/settings.json` (le créer s'il n'existe pas).
2. Copier en `~/.claude/settings.json.bak-AAAAMMJJ-HHMM`.
3. **Fusionner** : ajouter les hooks Coucou sans toucher aux hooks existants. Chemin de `coucou-hook` entre guillemets.
4. Montrer le diff à Louis, attendre son OK, écrire.
5. Bouton « Désinstaller les hooks » dans les réglages qui retire uniquement les entrées Coucou.

---

## 2. n8n (workflows de Louis)

- Réglages : URL de l'instance (probablement `https://n8nlouis.dcsys.tech`, **à confirmer avec Louis**) et clé API n8n (magasin de clés du système). La clé se crée dans n8n : Settings → n8n API.
- L'app joint n8n, pas l'inverse : **polling** toutes les 5 s de l'API publique :
  - noms des workflows : `GET /api/v1/workflows` (cache 10 min) ;
  - exécutions récentes : `GET /api/v1/executions` avec filtres de statut et `limit`.
- Mapping :
  - exécution en cours → tâche `working` (si l'API expose les exécutions en cours ; sinon n8n n'apparaît qu'aux erreurs et aux succès, c'est acceptable) ;
  - nouvelle exécution en erreur → alerte `error`, détail = nœud en échec + message (`GET /api/v1/executions/{id}?includeData=true`) ;
  - succès → mini-bonhomme `finished` 3 s en compact, **sans** ouvrir l'island (sinon trop de bruit), sauf réglage contraire.
- Boutons :
  - « Relancer » → endpoint de retry de l'API publique (vérifier sa présence et son chemin dans le playground de l'instance). S'il n'existe pas : ouvrir l'exécution dans n8n.
  - « Ouvrir dans n8n » → ouvrir `{URL}/workflow/{workflowId}/executions/{executionId}` dans le navigateur par défaut.
- Réglage « workflows suivis » : tous par défaut, liste à cocher.

---

## 3. Fichiers déposés

- Glisser-déposer sur la fenêtre de l'island. Copier les fichiers dans l'inbox de l'app — `%LOCALAPPDATA%\Coucou\inbox` (Windows), `~/.local/share/coucou/inbox` (Linux) — c'est la phase `uploading`.
- Vue `choose` :
  - **Poser une question dessus** → vue `prompt` avec une pastille du fichier. Envoi à l'API Claude (§5) : PDF en bloc `document`, images en bloc `image`, texte et code (≤ 200 Ko) en texte. Autres types : message « Je ne sais pas lire ce format. »
- Nettoyer l'inbox après 7 jours.

---

## 4. Attacher le bonhomme à une fenêtre

Non disponible dans cette version (Windows et Linux).

---

## 5. API Claude (recherche)

- `POST https://api.anthropic.com/v1/messages`, en-têtes `x-api-key`, `anthropic-version`, `content-type: application/json` (versions à vérifier dans la doc).
- Modèle par défaut : `claude-sonnet-5`, réglable dans les réglages. Vérifier la liste des modèles disponibles dans la doc.
- Outil de recherche web côté serveur de l'API : l'identifiant de type à jour est dans la doc (au moment d'écrire, `web_search_20250305`) ; `max_uses` 5.
- Prompt système (français) : répondre court, pour un affichage dans l'island, au format JSON strict :
  ```json
  { "title": "…", "items": [ { "label": "…", "detail": "…", "url": "…" } ], "note": "…" }
  ```
  3 items maximum. Si le JSON est invalide : afficher le texte brut (3 lignes max) dans la vue `result`.
- Contenu du message utilisateur : fichier (§3) + demande, ou demande seule (onglet Demander).
- Pendant l'appel : état `searching`, vue `searching`, texte scintillant. Réponse : état `finished`, vue `result`, émote Fier, son `finish`.
- Boutons du résultat : « Ouvrir » (premier lien, seulement s'il est en http ou https ; sinon le bouton est grisé), « Copier » (texte), « Fermer ».
- Erreur réseau ou clé invalide : état `error`, vue `note` avec la raison en une phrase et « Ouvre les réglages pour vérifier la clé ».

---

## 6. Envoi par e-mail

Non disponible dans cette version (Windows et Linux).

---

## 7. Autorisations système

Aucune autorisation particulière ni droit administrateur, sous Windows comme sous Linux. Sous Linux, les clés sont rangées dans le Secret Service (GNOME Keyring, KWallet, KeePassXC).
