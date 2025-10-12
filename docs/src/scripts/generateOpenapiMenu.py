import json
import os
from pathlib import Path


def sanitize_label(label: str) -> str:
    return label.strip()


def generate_sidebar(openapi: dict) -> dict:
    items = ["restapi"]

    valid_methods = {"get", "post", "put", "delete", "patch", "options", "head"}

    for path_key, path_item in openapi.get("paths", {}).items():
        for method, operation in path_item.items():
            if method not in valid_methods:
                continue

            operation_id = operation.get("operationId") or f"{method}_{path_key.replace('/', '_').replace('{', '_').replace('}', '_')}"
            label = sanitize_label(operation.get("summary")) if operation.get("summary") else f"{method.upper()} {path_key}"
            href = f"/docs/restapi#tag/crate/operation/{operation_id}"

            items.append({
                "type": "link",
                "label": label,
                "href": href
            })

    return {
        "apiSidebar": [
            {
                "type": "category",
                "label": "RestAPI",
                "items": items
            }
        ]
    }


def main():
    current_dir = Path(__file__).resolve().parent
    openapi_path = current_dir / "../../static/openapi.json"
    sidebar_output_path = current_dir / "../sidebars/apiSidebar.json"

    with open(openapi_path, "r", encoding="utf-8") as f:
        openapi = json.load(f)

    sidebar = generate_sidebar(openapi)

    os.makedirs(sidebar_output_path.parent, exist_ok=True)

    with open(sidebar_output_path, "w", encoding="utf-8") as f:
        json.dump(sidebar, f, indent=2)

    print(f"Sidebar generated at {sidebar_output_path}")


if __name__ == "__main__":
    main()
