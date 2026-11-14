# Source provenance

The uploaded `dmitbox-local.sh` is retained unchanged as `upstream/dmitbox-local-r3.sh`. Its SHA-256 is `c7eadda900dc8a708b9c37e814bd14c0c7d8b7ebeb70c2537c5d84d42c97c5af`.

The supplied review and two legacy test scripts are archived beside it. The new builder assembles only `src/` modules. It never executes or imports the archived implementation and requires no external baseline directory.

Existing upstream declarations are preserved in those original files. This component is integrated under `tools/dmitbox-local` in rustaller. It remains independently invoked and is not automatically applied during installation.
