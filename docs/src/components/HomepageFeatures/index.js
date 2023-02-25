import React from 'react';
import clsx from 'clsx';
import styles from './styles.module.css';

const FeatureList = [
  {
    title: 'The same history, everywhere',
    Svg: require('@site/static/img/undraw_docusaurus_mountain.svg').default,
    description: (
      <>
        <p>
          Atuin syncs your shell history between every single machine you own. Never write the same command twice!
        </p>
        <p>

        </p>
      </>
    ),
  },
  {
    title: 'Find it fast',
    Svg: require('@site/static/img/undraw_docusaurus_tree.svg').default,
    description: (
      <>
        Atuin gives you a speedy terminal search UI for your history, powered by configurable fuzzy search.
      </>
    ),
  },
  {
    title: 'Importers',
    Svg: require('@site/static/img/undraw_docusaurus_react.svg').default,
    description: (
      <>
        With importers for several shells + other history tools, you won't forget your past switching to Atuin
      </>
    ),
  },
];

const FeatureListBottom = [
  {
    title: 'Super Secure',
    offset: true,
    Svg: require('@site/static/img/undraw_docusaurus_mountain.svg').default,
    description: (
      <>
        With end-to-end encryption everywhere, nobody else can see your shell history
      </>
    ),
  },
  {
    title: 'No limits',
    Svg: require('@site/static/img/undraw_docusaurus_tree.svg').default,
    description: (
      <>
        Atuin handles anything from 1000 lines of history to a 1/4 million and beyond!
      </>
    ),
  },
];


function Feature({ Svg, title, description, offset }) {
  return (
    <div className={clsx('col col--4', offset && 'col--offset-2')}>
      <div className="text--center">
        <Svg className={styles.featureSvg} role="img" />
      </div>
      <div className="text--center padding-horiz--md">
        <h3>{title}</h3>
        <p>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures() {
  return (
    <section className={styles.features}>
      <div className="container">
        <div className="row">
          {FeatureList.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
        <div className="row">
          {FeatureListBottom.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}
