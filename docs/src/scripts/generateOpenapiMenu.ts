import * as fs from 'fs';
import { fileURLToPath } from 'url';
import path from 'path';

interface OpenAPIOperation {
    operationId?: string;
    summary?: string;
    [key: string]: any;
}

interface OpenAPIPaths {
    [path: string]: {
        [method: string]: OpenAPIOperation;
    };
}

interface OpenAPI {
    paths: OpenAPIPaths;
    [key: string]: any;
}

interface SidebarLink {
    type: 'link';
    label: string;
    href: string;
}

type SidebarElement = SidebarLink | string;

interface SidebarCategory {
    type: 'category';
    label: string;
    items: SidebarElement[];
}

interface Sidebar {
    apiSidebar: SidebarCategory[];
}

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const openApiPath = path.resolve(__dirname, '../../static/openapi.json');
const sidebarOutputPath = path.resolve(__dirname, '../sidebars/apiSidebar.json');

function sanitizeLabel(label: string): string {
    return label.trim();
}

function generateSidebar(openApi: OpenAPI): Sidebar {
    const items: SidebarElement[] = ["restapi"];

    for (const [pathKey, pathItem] of Object.entries(openApi.paths)) {
        for (const [method, operation] of Object.entries(pathItem)) {
            if (!['get', 'post', 'put', 'delete', 'patch', 'options', 'head'].includes(method)) {
                continue;
            }

            const operationId = operation.operationId || `${method}_${pathKey.replace(/[\/{}]/g, '_')}`;
            const label = operation.summary
                ? sanitizeLabel(operation.summary)
                : `${method.toUpperCase()} ${pathKey}`;

            const href = `/docs/restapi#tag/crate/operation/${operationId}`;

            items.push({
                type: 'link',
                label,
                href,
            });
        }
    }

    return {
        apiSidebar: [
            {
                type: 'category',
                label: 'RestAPI',
                items,
            },
        ],
    };
}

function main() {
    const raw = fs.readFileSync(openApiPath, 'utf-8');
    const openApi: OpenAPI = JSON.parse(raw);

    const sidebar = generateSidebar(openApi);

    fs.mkdirSync(path.dirname(sidebarOutputPath), { recursive: true });
    fs.writeFileSync(sidebarOutputPath, JSON.stringify(sidebar, null, 2));

    console.log(`Sidebar generated at ${sidebarOutputPath}`);
}

main();
