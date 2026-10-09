# Shop

Shop sells books online. It has two services and one shared library.
Run `docker compose up` from this folder to start both services.

## Services

<!-- memoria:import src="services/api/README.md#summary" -->
### Orders API

The orders API accepts orders over HTTP and keeps them in memory. It depends on no other service.
<!-- /memoria:import -->

<!-- memoria:import src="services/web/README.md#summary" -->
### Web shop

The web shop shows the book list and the cart. It sends each order to the orders API.
<!-- /memoria:import -->

## Libraries

- [libs/shared](libs/shared/README.md) holds code that more than one service uses.
