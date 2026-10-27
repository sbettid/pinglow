import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import BrowserOnly from '@docusaurus/BrowserOnly';
import { useColorMode } from '@docusaurus/theme-common';

export default function RedocWrapper(): ReactNode {
    const containerRef = useRef<HTMLDivElement>(null);
    const [error, setError] = useState<Error | null>(null);
    const { colorMode } = useColorMode();

    useEffect(() => {
        let cancelled = false;
        let unmount: (() => void) | undefined;

        window.localStorage.setItem('redoc.colorMode', colorMode);

        void import('redoc/bundles/redoc.standalone.js')
            .then(({ init }) => {
                if (!containerRef.current || cancelled) {
                    return;
                }

                const root = init(
                    '/pinglow/openapi.json',
                    {
                        hideDownloadButtons: true,
                        disableTelemetry: true,
                    },
                    containerRef.current,
                );
                unmount = () => root.unmount();
            })
            .catch((cause: unknown) => {
                if (cancelled) {
                    return;
                }

                const loadError = cause instanceof Error ? cause : new Error(String(cause));
                console.error('Unable to load API documentation', loadError);
                setError(loadError);
            });

        return () => {
            cancelled = true;
            unmount?.();
        };
    }, [colorMode]);

    return (
        <BrowserOnly fallback={<div>Loading API documentation...</div>}>
            {() => error
                ? <div role="alert">Unable to load API documentation.</div>
                : <div ref={containerRef} className="redocApiDocs" />
            }
        </BrowserOnly>
    );
}
