# Cloud architecture icons

These SVG files are third-party product artwork, not original yyeditor artwork.
The workspace's GPL license does not replace the providers' rights in their icons.
They are included in yydraw for making architecture diagrams. Preserve the original
appearance and identify the service with its label. Do not use the product icons
as a logo for yydraw or another product. Source and terms checked 2026-10-20:

- AWS: https://aws.amazon.com/architecture/icons/
  Official icon package 2026-07-31: all service, resource, category and group SVG
  artwork (859 entries). Duplicate service/category sizes use the 48 px original.
  AWS permits customers and partners
  to use its toolkits and assets to create architecture diagrams.
- Microsoft Azure: https://learn.microsoft.com/en-us/azure/architecture/icons/
  Azure Public Service Icons V24: all 714 SVG icons, with their original categories.
  Microsoft permits copying, distribution and display for architectural diagrams,
  training materials and documentation. All other rights are reserved by Microsoft.
  Do not crop, flip, rotate, distort or change the icon shape.
- Google Cloud: https://cloud.google.com/icons
  All official SVG downloads: category-icons.zip (26), core-products-icons.zip (19),
  google-cloud-legacy-icons.zip (216). Legacy artwork is explicitly separated.
  The library supplies these product icons for diagrams and technical documentation.
  Google Cloud product names and icons remain the property of Google.

The source URLs are also stored in each document's embedded asset metadata.
SVG artwork is kept unmodified inside catalog.json.gz; the renderer normalizes it
in memory for display. Names and navigation groups are catalog metadata. AWS and
Azure categories follow their distributions; Google service navigation groups are
maintained by yydraw because its archives do not supply category assignments.
Download URLs, package SHA-256 hashes and entry counts are in catalog-sources.json.
Rebuild with python tools/cloud-icons/fetch.py. Original archives remain in target/
and are not needed at runtime. Only placed artwork is embedded in user documents.
Additional packs may be imported locally by the user.

## Original business/IT icons

`business-icons.json` is original yydraw artwork, licensed GPL-3.0-or-later with
the repository. Its 376 icons illustrate business and IT concepts. Product-named
icons are original concept illustrations, not official product or provider logos.
They are independent of the third-party cloud catalog above. Rebuild with
`python tools/business-icons/generate.py`; the categorized inventory and search
terms are in `docs/yydraw-business-icons.md`.
