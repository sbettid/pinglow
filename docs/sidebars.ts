import type { SidebarsConfig } from '@docusaurus/plugin-content-docs';


import apiSidebar from './src/sidebars/apiSidebar.json';

const sidebars: SidebarsConfig = {

  documentationSidebar: [
    'documentation',
    'architecture',
    {
      type: 'category',
      label: 'Deployment',
      items: [
        'deployment/requirements',
        'deployment/deployment'
      ],
    },
    {
      type: 'category',
      label: 'Concepts',
      items: [
        'concepts/checks-scripts',
        'concepts/continuous-deployment',
        'concepts/notifications'
      ],
    },
    ...apiSidebar.apiSidebar
  ],
};

export default sidebars;
