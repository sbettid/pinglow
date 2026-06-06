import { RedocStandalone, RedocStandaloneProps } from 'redoc';
import { useColorMode } from '@docusaurus/theme-common';
import React, { useEffect, useState, ReactNode } from 'react';
import BrowserOnly from '@docusaurus/BrowserOnly';

export default function RedocWrapper(): ReactNode {
    const { colorMode } = useColorMode();

    // Define the theme using RedocStandaloneProps
    const lightTheme: RedocStandaloneProps['options']['theme'] = {
        colors: {
            primary: {
                main: '#31c0df',
            },
            text: {
                primary: '#1c1e21',
                secondary: '#586069',
            },
            http: {
                get: '#6bbd5b',
                post: '#248fb2',
                put: '#fca130',
                delete: '#f93e3e',
            },
        },
        sidebar: {
            backgroundColor: '#ffffff',
            width: "0px"
        },
    };

    const darkTheme: RedocStandaloneProps['options']['theme'] = {
        colors: {
            primary: {
                main: '#31c0df',
            },
            text: {
                primary: '#ffffff',
                secondary: '#cccccc',
            },
            http: {
                get: '#6bbd5b',
                post: '#248fb2',
                put: '#fca130',
                delete: '#f93e3e',
            },
        },
        sidebar: {
            backgroundColor: '#1e1e1e',
            width: "0px"
        },
    };

    return (
        <BrowserOnly fallback={<div>Loading API documentation...</div>}>
            {() => {
                const { RedocStandalone } = require('redoc');

                return (
                    <RedocStandalone
                        specUrl="/pinglow/openapi.json"
                        options={{
                            nativeScrollbars: true,
                            hideDownloadButton: true,
                            theme: colorMode === 'dark' ? darkTheme : lightTheme,
                        }}
                    />
                );
            }}
        </BrowserOnly>
    );
}
