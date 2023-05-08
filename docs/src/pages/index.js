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
      <link rel="me" href="https://hachyderm.io/@atuin" />

      <div className="container">
        <h1 className="hero__title">Making your shell <b className={styles.magical}>magical</b></h1>
        <p className="hero__subtitle">Sync, search and backup shell history with Atuin</p>
        <div className={styles.buttons}>
          <Link
            className="button button--primary button--lg"
            to="/docs">
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
            <div className="row">
              <img src="/img/screenshot.png" className="col col--8" />
              <div className="col col--4">
                <p>Atuin is a command-line tool that enables you to make better use of your shell, by giving ctrl-r superpowers.</p>
                <p>Every line you write is stored - ready to be queried and run again at any point, from any machine you wish. Never forget again!</p>
                <p>Sync your history between all of your machines, and search it from anywhere</p>
              </div>
            </div>

            <div className="row" style={{ paddingTop: "18px", alignItems: "center" }}>
              <div className="col col--4">
                <p>Generate statistics from your shell history, such as this activity graph</p>
              </div>
              <div className="col col--8">
                <img src="https://api.atuin.sh/img/ellie.png?token=0722830c382b42777bdb652da5b71efb61d8d387" />
              </div>
            </div>

            <HomepageFeatures />
          </div>
        </section>
      </main>
    </Layout >
  );
}
