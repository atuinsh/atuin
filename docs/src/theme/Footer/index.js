import React from 'react';
import Footer from '@theme-original/Footer';

export default function FooterWrapper(props) {
  return (
    <>
      <Footer {...props} />
      <script defer data-domain="atuin.sh" src="https://plausible.io/js/script.js"></script>
    </>
  );
}
