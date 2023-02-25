import React from 'react';
import clsx from 'clsx';
import Link from '@docusaurus/Link';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Layout from '@theme/Layout';
import HomepageFeatures from '@site/src/components/HomepageFeatures';

import styles from './index.module.css';

function HomepageHeader() {
  const { siteConfig } = useDocusaurusContext();
  return (
    <header className={clsx('hero', styles.heroBanner)}>
      <link rel="icon" href="data:image/svg+xml,<svg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 100 100%22><text y=%22.9em%22 font-size=%2290%22>🐢</text></svg>" />

      <div className="container">
        <h1 className="hero__title">Making your shell <b className={styles.magical}>magical</b></h1>
        <p className="hero__subtitle">Sync, search and backup shell history with Atuin</p>
        <div className={styles.buttons}>
          <Link
            className="button button--primary button--lg"
            to="/docs/setup">
            Get Started
          </Link>
        </div>
      </div>
    </header>
  );
}

export default function Home() {
  const { siteConfig } = useDocusaurusContext();
  return (
    <Layout
      title={`Magical Shell History`}>
      <HomepageHeader />
      <main>
        <section className={styles.whatis}>
          <div className="container">
            <center><h1>What is <b>Atuin</b>?</h1></center>

            <HomepageFeatures />
          </div>
        </section>
      </main>
    </Layout >
  );
}
