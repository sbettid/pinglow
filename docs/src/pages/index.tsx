import type { ReactNode } from 'react';
import clsx from 'clsx';
import Link from '@docusaurus/Link';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Layout from '@theme/Layout';
import Heading from '@theme/Heading';

import styles from './index.module.css';

export default function Home(): ReactNode {
  const { siteConfig } = useDocusaurusContext();
  return (
    <Layout
      title={`${siteConfig.title}`}
      description="Description will go into a meta tag in <head />">
      <main className={styles.hero}>
        <div className={styles.container}>
          <img className={styles.pinglowLogo} src='img/pinglow_full_transparent_nobuffer.png' />
          <p className={styles.subtitle}>
            A prototype of a Kubernetes ready monitoring engine
          </p>
          <Link
            className={`button button--primary button--lg ${styles.readTheDocsLink}`}
            to="/docs/documentation"
          >
            Read the docs
          </Link>
        </div>
      </main>
    </Layout>
  );
}
