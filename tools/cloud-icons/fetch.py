"""Build the bundled catalog from the providers' official SVG distributions.

Run from the repository root. Downloads stay under target/; only the deterministic
compressed catalog and its provenance manifest belong in source control.
"""
import gzip
import hashlib
import json
from pathlib import Path
import re
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / "target/yydraw-assets"
OUT = ROOT / "crates/yy-draw/assets"
SOURCES = [
    ("AWS", "aws", "https://aws.amazon.com/architecture/icons/", "https://d1.awsstatic.com/onedam/marketing-channels/website/public/shared/architecture-icon-release/Icon-package_07312026.5846e92413caa21490223536cc97f1269e44fa92.zip"),
    ("Azure", "azure", "https://learn.microsoft.com/en-us/azure/architecture/icons/", "https://arch-center.azureedge.net/icons/Azure_Public_Service_Icons_V24.zip"),
    ("Google Cloud", "google-category", "https://cloud.google.com/icons", "https://services.google.com/fh/files/misc/category-icons.zip"),
    ("Google Cloud", "google", "https://cloud.google.com/icons", "https://services.google.com/fh/files/misc/core-products-icons.zip"),
    ("Google Cloud", "google-legacy", "https://cloud.google.com/icons", "https://services.google.com/fh/files/misc/google-cloud-legacy-icons.zip"),
]


def google_category(name):
    # Google's legacy archive has service folders but no category metadata.
    # These navigation groups are maintained by yydraw; artwork stays untouched.
    name = name.lower().replace(" ", "_")
    rules = [
        ("AI and ML", ("ai", "automl", "vertex", "agent", "tensorflow", "natural_language", "speech", "translate", "vision", "recommend", "talent", "hypercomputer", "inference")),
        ("Security", ("security", "threat", "mandiant", "armor", "kms", "key_", "iam", "identity", "beyondcorp", "certificate", "access_context", "assured", "recaptcha", "confidential", "private_ca", "webrisk", "web_security", "policy_", "secret", "access_", "binary_authorization")),
        ("Databases", ("sql", "spanner", "bigtable", "firestore", "datastore", "memorystore", "alloydb", "database")),
        ("Data analytics", ("bigquery", "data", "pubsub", "pub_sub", "composer", "looker", "analytics", "preparation", "cloud_life_sciences")),
        ("Storage", ("storage", "filestore", "persistent_disk", "transfer", "backup")),
        ("Networking", ("network", "vpc", "cdn", "dns", "load_balanc", "interconnect", "router", "vpn", "nat", "traffic", "connect", "cloud_domains", "service_directory")),
        ("Containers", ("gke", "kubernetes", "anthos", "container", "service_mesh", "config_management", "distributed_cloud")),
        ("Compute", ("compute", "engine", "run", "function", "batch", "bare_metal", "vmware", "gpu", "tpu", "migrate")),
        ("APIs and integration", ("api", "apigee", "endpoint", "eventarc", "workflow", "scheduler", "tasks", "iot")),
        ("Developer tools", ("build", "code", "deploy", "source", "artifact", "debug", "tools", "workstation", "sdk", "powershell", "test", "cloud_shell")),
        ("Operations", ("monitor", "logging", "trace", "profiler", "error", "debugger", "operation")),
    ]
    for category, keywords in rules:
        if any((word in name.split("_") if len(word) <= 2 else word in name) for word in keywords):
            return category
    return "Management and other services"


def describe(vendor, archive, path):
    parts = path.split("/")
    name = Path(path).stem
    if vendor == "AWS":
        # SVG sizes are duplicate representations of the same artwork. Keep 48px
        # service/category artwork; resource/group icons are supplied at one size.
        if parts[0].startswith("Architecture-Service") and "/48/" not in path:
            return None
        if parts[0].startswith("Category") and "/Arch-Category_48/" not in path:
            return None
        category = "Groups" if parts[0].startswith("Architecture-Group") else "Categories" if parts[0].startswith("Category") else re.sub(r"^(Arch|Res)_", "", parts[1])
        name = re.sub(r"^(Arch|Res|Arch-Category)_", "", name)
        name = re.sub(r"_(16|32|48|64)$", "", name)
    elif vendor == "Azure":
        category = parts[-2]
        name = re.sub(r"^\d+-icon-service-", "", name)
    elif archive == "google-category":
        category = "Categories"
        name = parts[1]
    elif archive == "google":
        name = parts[1]
        category = google_category(name)
    else:
        category = "Legacy / " + google_category(name)
    return category.replace("-", " "), name.replace("_", " ").replace("-", " ")


def main():
    CACHE.mkdir(parents=True, exist_ok=True)
    entries, provenance = [], []
    for vendor, archive, source, url in SOURCES:
        dest = CACHE / (archive + ".zip")
        if not dest.exists():
            print("Downloading", url, flush=True)
            urllib.request.urlretrieve(url, dest)
        count = 0
        with zipfile.ZipFile(dest) as z:
            for path in sorted(z.namelist()):
                if not path.lower().endswith(".svg") or "__MACOSX" in path or Path(path).name.startswith("."):
                    continue
                info = describe(vendor, archive, path)
                if info is None:
                    continue
                category, name = info
                entries.append(dict(vendor=vendor, category=category, name=name, source=source,
                                    file=path, svg=z.read(path).decode("utf-8-sig")))
                count += 1
        provenance.append(dict(vendor=vendor, source=source, download=url,
                               sha256=hashlib.sha256(dest.read_bytes()).hexdigest(), icons=count))
        print(vendor, archive, count, flush=True)
    entries.sort(key=lambda e: (e["vendor"], e["category"].casefold(), e["name"].casefold(), e["file"]))
    OUT.joinpath("catalog.json.gz").write_bytes(gzip.compress(json.dumps(entries, ensure_ascii=False, separators=(",", ":")).encode(), mtime=0))
    OUT.joinpath("catalog-sources.json").write_text(json.dumps(provenance, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print("Total", len(entries))


if __name__ == "__main__":
    main()
