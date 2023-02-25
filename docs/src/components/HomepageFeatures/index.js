import React from 'react';
import clsx from 'clsx';
import styles from './styles.module.css';

const FeatureList = [
  {
    title: 'History sync',
    Svg: require('@site/static/img/undraw_docusaurus_mountain.svg').default,
    description: (
      <>
        <ul>
          <li>Sync your shell history to all of your machines, wherever they are</li>
          <li>End-to-end encrypted - nobody can see your data but you</li>
          <li>Securely backed up - never lose a command again</li>
        </ul>
      </>
    ),
  },
  {
    title: 'Find it fast',
    Svg: require('@site/static/img/undraw_docusaurus_tree.svg').default,
    description: (
      <>
        <ul>
          <li>Speedy terminal search UI</li>
          <li>Configurable search method - fuzzy, prefix, etc</li>
          <li>Easily search and filter by session, directory, or machine</li>
          <li>Powerful command line search for integration with other tools</li>
        </ul>
      </>
    ),
  },
  {
    title: 'All the data',
    Svg: require('@site/static/img/undraw_docusaurus_react.svg').default,
    description: (
      <>
        <ul>
          <li>History stored in a SQLite DB, making stats and analysis easy</li>
          <li>Log exit code, directory, hostname, session, command duration, etc</li>
          <li>Import old history from a number of shells or history tools</li>
        </ul>
      </>
    ),
  },
];

function Feature({ Svg, title, description }) {
  return (
    <div className={clsx('col col--4')}>
      <div className={"padding-horiz--md", styles.whatisfeature}>
        <h3>{title}</h3>
        <p>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures() {
  return (
    <section className={styles.features}>
      <div className={"container"}>
        <div className="row">
          {FeatureList.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}
